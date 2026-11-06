#[cfg(unix)]
fn main() {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::process;

    use agentenv::sudo::askpass_channel::{FIELD_LIMIT, SUDO};
    use zeroize::Zeroizing;

    fn fail() -> ! {
        process::exit(1)
    }

    fn write_field(stream: &mut UnixStream, value: &[u8]) -> std::io::Result<()> {
        stream.write_all(&SUDO.encode_field(value)?)
    }

    let mut args = std::env::args_os().skip(1);
    let first = args.next();
    if first.as_deref() == Some(std::ffi::OsStr::new("--identity")) {
        if args.next().is_some() {
            fail();
        }
        println!("{}", agentenv::sudo::deploy::expected_identity());
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
    if session.is_empty() || session.len() > FIELD_LIMIT {
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
    let mut header = vec![0_u8; SUDO.reply_header_len()];
    if stream.read_exact(&mut header).is_err() {
        fail();
    }
    let Ok(length) = SUDO.secret_len(&header, agentenv::sudo::SUDO_PASSWORD_LIMIT) else {
        fail();
    };
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
        println!("{}", agentenv::sudo::deploy::expected_identity());
        return;
    }
    std::process::exit(1);
}
