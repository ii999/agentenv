use std::io::{self, IsTerminal, Write};
use std::time::{Duration, Instant};

use zeroize::Zeroizing;

use super::{input::Decoder, Input};

pub(super) struct Console {
    output: Box<dyn Write>,
    raw: bool,
    input_mode: Option<libc::termios>,
    decoder: Decoder,
    dimensions: Option<(u16, u16)>,
}

impl Console {
    pub fn open() -> io::Result<Self> {
        let output: Box<dyn Write> = if io::stderr().is_terminal() {
            Box::new(io::stderr())
        } else {
            Box::new(std::fs::OpenOptions::new().write(true).open("/dev/tty")?)
        };
        let mut console = Self {
            output,
            raw: false,
            input_mode: None,
            decoder: Decoder::new(),
            dimensions: None,
        };
        if !crossterm::terminal::is_raw_mode_enabled()? {
            crossterm::terminal::enable_raw_mode()?;
            console.raw = true;
        }
        let mut mode = std::mem::MaybeUninit::uninit();
        if unsafe { libc::tcgetattr(libc::STDIN_FILENO, mode.as_mut_ptr()) } == -1 {
            return Err(io::Error::last_os_error());
        }
        let mut mode = unsafe { mode.assume_init() };
        console.input_mode = Some(mode);
        // Return promptly after readiness without changing descriptor flags
        // that stdin may share with the terminal's output streams.
        mode.c_cc[libc::VMIN] = 0;
        mode.c_cc[libc::VTIME] = 0;
        if unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &mode) } == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(console)
    }

    pub fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.output.write_all(bytes)?;
        self.output.flush()
    }

    pub fn read(&mut self, timeout: Duration) -> io::Result<Option<Input>> {
        let deadline = Instant::now() + timeout;
        loop {
            let size = self.size()?;
            if self.dimensions != Some(size) {
                self.dimensions = Some(size);
                return Ok(Some(Input::Redraw));
            }
            self.decoder.idle();
            if let Some(input) = self.decoder.pop() {
                return Ok(Some(input));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(None);
            }
            let mut descriptor = libc::pollfd {
                fd: libc::STDIN_FILENO,
                events: libc::POLLIN,
                revents: 0,
            };
            let polled = unsafe {
                libc::poll(
                    &mut descriptor,
                    1,
                    remaining.as_millis().max(1).min(i32::MAX as u128) as i32,
                )
            };
            if polled == -1 {
                let error = io::Error::last_os_error();
                return if error.kind() == io::ErrorKind::Interrupted {
                    Ok(None)
                } else {
                    Err(error)
                };
            }
            if descriptor.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "terminal input closed",
                ));
            }
            if descriptor.revents & libc::POLLIN != 0 {
                let mut bytes = Zeroizing::new([0_u8; 1024]);
                let count = unsafe {
                    libc::read(libc::STDIN_FILENO, bytes.as_mut_ptr().cast(), bytes.len())
                };
                if count == -1 {
                    let error = io::Error::last_os_error();
                    if matches!(
                        error.kind(),
                        io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                    ) {
                        return Ok(None);
                    }
                    return Err(error);
                }
                if count == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "terminal input closed",
                    ));
                }
                self.decoder.bytes(&bytes[..count as usize])?;
            }
        }
    }

    pub fn size(&self) -> io::Result<(u16, u16)> {
        crossterm::terminal::size()
    }

    pub fn restore(&mut self) -> io::Result<()> {
        let polling = if let Some(mode) = self.input_mode {
            if unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &mode) } == -1 {
                Err(io::Error::last_os_error())
            } else {
                self.input_mode = None;
                Ok(())
            }
        } else {
            Ok(())
        };
        let raw = if self.raw {
            let result = crossterm::terminal::disable_raw_mode();
            if result.is_ok() {
                self.raw = false;
                self.input_mode = None;
            }
            result
        } else {
            Ok(())
        };
        polling.and(raw)
    }
}

impl Drop for Console {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}
