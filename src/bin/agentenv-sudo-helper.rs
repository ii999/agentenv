#[cfg(unix)]
fn main() {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::process;

    use zeroize::Zeroizing;

    const MAGIC: &[u8; 5] = b"AGES\x01";
    const MAX_FIELD: usize = 4096;

    fn fail() -> ! {
        process::exit(1)
    }

    fn write_field(stream: &mut UnixStream, value: &[u8]) -> std::io::Result<()> {
        if value.is_empty() || value.len() > MAX_FIELD || value.len() > u16::MAX as usize {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid",
            ));
        }
        stream.write_all(MAGIC)?;
        stream.write_all(&(value.len() as u16).to_be_bytes())?;
        stream.write_all(value)
    }

    let mut args = std::env::args_os().skip(1);
    let first = args.next();
    if first.as_deref() == Some(std::ffi::OsStr::new("--identity")) {
        if args.next().is_some() {
            fail();
        }
        println!(
            "agentenv-sudo-helper {} {}",
            agentenv::sudo::PROTOCOL_VERSION,
            env!("CARGO_PKG_VERSION")
        );
        return;
    }
    if first.as_deref() == Some(std::ffi::OsStr::new("--serve")) {
        if args.next().is_some() {
            fail();
        }
        let Ok(runtime) = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        else {
            process::exit(9);
        };
        let result = runtime.block_on(agentenv::sudo::remote::serve(
            tokio::io::stdin(),
            tokio::io::stdout(),
        ));
        // A pending pipe read must not keep check mode or cancellation alive
        // after the session has closed its process and protocol resources.
        runtime.shutdown_timeout(std::time::Duration::from_millis(100));
        process::exit(result.map_or_else(|error| error.exit_code(), |()| 0));
    }
    let Some(prompt) = first.and_then(|value| value.into_string().ok()) else {
        fail();
    };
    if args.next().is_some() {
        fail();
    }
    let Some(socket) = std::env::var_os("AGENTENV_SUDO_SOCKET") else {
        fail();
    };
    let Ok(session) = std::env::var("AGENTENV_SUDO_SESSION") else {
        fail();
    };
    if session.is_empty() || session.len() > MAX_FIELD {
        fail();
    }
    let prompt_prefix = format!("agentenv sudo [{session}] password for ");
    if !prompt.starts_with(&prompt_prefix) || !prompt.ends_with(':') {
        fail();
    }
    let Ok(mut stream) = UnixStream::connect(socket) else {
        fail();
    };
    if write_field(&mut stream, session.as_bytes()).is_err()
        || write_field(&mut stream, prompt.as_bytes()).is_err()
    {
        fail();
    }
    let mut magic = [0_u8; 5];
    let mut status = [0_u8; 1];
    let mut length = [0_u8; 2];
    if stream.read_exact(&mut magic).is_err()
        || &magic != MAGIC
        || stream.read_exact(&mut status).is_err()
        || stream.read_exact(&mut length).is_err()
        || status[0] != 0
    {
        fail();
    }
    let length = u16::from_be_bytes(length) as usize;
    if length == 0 || length > agentenv::sudo::SUDO_PASSWORD_LIMIT {
        fail();
    }
    let mut password = Zeroizing::new(vec![0_u8; length]);
    if stream.read_exact(&mut password).is_err() {
        fail();
    }
    let mut trailing = [0_u8; 1];
    if stream.read(&mut trailing).ok() != Some(0) {
        fail();
    }
    let mut stdout = std::io::stdout().lock();
    if stdout.write_all(&password).is_err()
        || stdout.write_all(b"\n").is_err()
        || stdout.flush().is_err()
    {
        fail();
    }
}

#[cfg(not(unix))]
fn main() {
    if std::env::args().skip(1).collect::<Vec<_>>() == ["--identity"] {
        println!(
            "agentenv-sudo-helper {} {}",
            agentenv::sudo::PROTOCOL_VERSION,
            env!("CARGO_PKG_VERSION")
        );
        return;
    }
    std::process::exit(1);
}
