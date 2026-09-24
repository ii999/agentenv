//! Windows-only process and IPC boundaries. Pipe names are routing metadata,
//! not bearer secrets: the DACL and kernel-reported peer identities authorize
//! each connection. Handles are non-inheritable; providers inherit no channel.

use std::ffi::{c_void, OsString};
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};

use tokio::net::windows::named_pipe::{
    ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
};
use windows_sys::Win32::Foundation::{LocalFree, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
};
use windows_sys::Win32::Security::Cryptography::{
    BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, TokenUser, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::Storage::FileSystem::SECURITY_ANONYMOUS;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Pipes::{GetNamedPipeClientProcessId, GetNamedPipeServerProcessId};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, QueryFullProcessImageNameW,
    PROCESS_QUERY_LIMITED_INFORMATION,
};

pub(crate) fn random(bytes: &mut [u8]) -> io::Result<()> {
    let size = u32::try_from(bytes.len()).map_err(|_| denied())?;
    // SAFETY: bytes is a writable buffer of size bytes; system RNG needs no handle.
    if unsafe {
        BCryptGenRandom(
            null_mut(),
            bytes.as_mut_ptr(),
            size,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    } < 0
    {
        return Err(denied());
    }
    Ok(())
}

pub(crate) fn nonce() -> io::Result<String> {
    let mut bytes = [0; 24];
    random(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn owned(handle: HANDLE) -> io::Result<OwnedHandle> {
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: caller transfers a newly created, valid owned kernel handle.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

fn denied() -> io::Error {
    io::Error::from(io::ErrorKind::PermissionDenied)
}

struct LocalAllocation(*mut c_void);
impl Drop for LocalAllocation {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}

/// One local, one-instance byte pipe, with an explicit protected current-user
/// DACL. A name collision fails; no fallback to a pre-existing listener.
pub(crate) fn private_pipe(purpose: &str) -> io::Result<(String, NamedPipeServer)> {
    let name = format!(
        r"\\.\pipe\agentenv-{purpose}-{}-{}",
        std::process::id(),
        nonce()?
    );
    let mut token = null_mut();
    // SAFETY: all output pointers refer to live storage; allocations below
    // remain live through CreateNamedPipe, which copies the descriptor.
    unsafe {
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(denied());
        }
        let token = owned(token)?;
        let mut size = 0;
        GetTokenInformation(token.as_raw_handle(), TokenUser, null_mut(), 0, &mut size);
        if size == 0 || size > 65536 {
            return Err(denied());
        }
        // TOKEN_USER includes pointers and needs pointer-aligned storage.
        let mut storage = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
        if GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            storage.as_mut_ptr().cast(),
            size,
            &mut size,
        ) == 0
        {
            return Err(denied());
        }
        let user = &*storage.as_ptr().cast::<TOKEN_USER>();
        let mut sid = null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut sid) == 0 {
            return Err(denied());
        }
        let _sid = LocalAllocation(sid.cast());
        let mut len = 0;
        while *sid.add(len) != 0 {
            len += 1;
        }
        let sid = String::from_utf16(std::slice::from_raw_parts(sid, len)).map_err(|_| denied())?;
        let sddl: Vec<u16> = format!("D:P(A;;GA;;;{sid})")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut descriptor = null_mut();
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut descriptor,
            null_mut(),
        ) == 0
        {
            return Err(denied());
        }
        let _descriptor = LocalAllocation(descriptor);
        let mut attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        let pipe = ServerOptions::new()
            .first_pipe_instance(true)
            .max_instances(1)
            .reject_remote_clients(true)
            .create_with_security_attributes_raw(
                &name,
                (&mut attributes as *mut SECURITY_ATTRIBUTES).cast(),
            )?;
        Ok((name, pipe))
    }
}

pub(crate) fn client_pid(pipe: &NamedPipeServer) -> io::Result<u32> {
    let mut pid = 0;
    if unsafe { GetNamedPipeClientProcessId(pipe.as_raw_handle(), &mut pid) } == 0 || pid == 0 {
        return Err(denied());
    }
    Ok(pid)
}

pub(crate) fn connect(name: &str, expected_server: u32) -> io::Result<NamedPipeClient> {
    let Some(suffix) = name.strip_prefix(r"\\.\pipe\agentenv-") else {
        return Err(denied());
    };
    if expected_server == 0
        || suffix.is_empty()
        || !suffix
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err(denied());
    }
    let pipe = ClientOptions::new()
        .security_qos_flags(SECURITY_ANONYMOUS)
        .open(name)?;
    let mut pid = 0;
    if unsafe { GetNamedPipeServerProcessId(pipe.as_raw_handle(), &mut pid) } == 0
        || pid != expected_server
    {
        return Err(denied());
    }
    Ok(pipe)
}

/// Retain the process handle while inspecting it so the process id cannot be
/// recycled into a different process during authorization.
pub(crate) fn verify_process(pid: u32, parent: u32, executable: &Path) -> io::Result<OwnedHandle> {
    let process = owned(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) })?;
    if parent_pid(pid)? != parent {
        return Err(denied());
    }
    let mut path = vec![0u16; 32768];
    let mut size = path.len() as u32;
    if unsafe {
        QueryFullProcessImageNameW(process.as_raw_handle(), 0, path.as_mut_ptr(), &mut size)
    } == 0
    {
        return Err(denied());
    }
    path.truncate(size as usize);
    let actual = std::fs::canonicalize(PathBuf::from(OsString::from_wide(&path)))?;
    let expected = std::fs::canonicalize(executable)?;
    if actual != expected {
        return Err(denied());
    }
    Ok(process)
}

pub(crate) fn parent_pid(pid: u32) -> io::Result<u32> {
    let snapshot = owned(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) })?;
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
    let mut found = unsafe { Process32FirstW(snapshot.as_raw_handle(), &mut entry) };
    while found != 0 {
        if entry.th32ProcessID == pid {
            return Ok(entry.th32ParentProcessID);
        }
        found = unsafe { Process32NextW(snapshot.as_raw_handle(), &mut entry) };
    }
    Err(denied())
}

/// Closing the sole job handle kills the resolver and every provider child.
/// Assignment happens before the parent sends the request which authorizes
/// the resolver to start a provider, avoiding a spawn/assignment escape race.
pub(crate) struct Job(OwnedHandle);
impl Job {
    pub(crate) fn new() -> io::Result<Self> {
        let job = owned(unsafe { CreateJobObjectW(null(), null()) })?;
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if unsafe {
            SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        } == 0
        {
            return Err(denied());
        }
        Ok(Self(job))
    }
    pub(crate) fn assign(&self, child: &impl AsRawHandle) -> io::Result<()> {
        if unsafe { AssignProcessToJobObject(self.0.as_raw_handle(), child.as_raw_handle()) } == 0 {
            return Err(denied());
        }
        Ok(())
    }
}
