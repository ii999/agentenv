use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::io::AsRawHandle;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyModifiers};
use windows_sys::Win32::Foundation::{HANDLE, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Console::*;
use windows_sys::Win32::System::Threading::WaitForSingleObject;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    VK_DELETE, VK_DOWN, VK_END, VK_F2, VK_HOME, VK_LEFT, VK_RIGHT, VK_UP,
};
use zeroize::Zeroizing;

use super::{input::Decoder, Input};

pub(super) struct Console {
    input: File,
    output: File,
    input_mode: u32,
    output_mode: u32,
    restore_input: bool,
    restore_output: bool,
    decoder: Decoder,
}

impl Console {
    pub fn open() -> io::Result<Self> {
        let input = OpenOptions::new().read(true).write(true).open("CONIN$")?;
        let output = OpenOptions::new().read(true).write(true).open("CONOUT$")?;
        let input_mode = mode(&input)?;
        let output_mode = mode(&output)?;
        let mut console = Self {
            input,
            output,
            input_mode,
            output_mode,
            restore_input: false,
            restore_output: false,
            decoder: Decoder::new(),
        };
        set_mode(
            &console.input,
            (input_mode
                | ENABLE_VIRTUAL_TERMINAL_INPUT
                | ENABLE_WINDOW_INPUT
                | ENABLE_EXTENDED_FLAGS)
                & !(ENABLE_ECHO_INPUT
                    | ENABLE_LINE_INPUT
                    | ENABLE_PROCESSED_INPUT
                    | ENABLE_QUICK_EDIT_MODE),
        )?;
        console.restore_input = true;
        set_mode(
            &console.output,
            output_mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING | ENABLE_PROCESSED_OUTPUT,
        )?;
        console.restore_output = true;
        Ok(console)
    }

    pub fn read(&mut self, timeout: Duration) -> io::Result<Option<Input>> {
        let deadline = Instant::now() + timeout;
        loop {
            self.decoder.idle();
            if let Some(event) = self.decoder.pop() {
                return Ok(Some(event));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(None);
            }
            // No other reader consumes this console while the editor is open.
            let wait = unsafe {
                WaitForSingleObject(
                    handle(&self.input),
                    remaining.as_millis().min(u128::from(u32::MAX - 1)) as u32,
                )
            };
            match wait {
                WAIT_TIMEOUT => return Ok(None),
                WAIT_FAILED => return Err(io::Error::last_os_error()),
                WAIT_OBJECT_0 => {}
                _ => {
                    return Err(io::Error::other(
                        "the console input wait returned an unexpected result",
                    ))
                }
            }
            let mut record = INPUT_RECORD::default();
            let mut count = 0;
            if unsafe { ReadConsoleInputW(handle(&self.input), &mut record, 1, &mut count) } == 0 {
                return Err(io::Error::last_os_error());
            }
            if count == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "console input closed",
                ));
            }
            match u32::from(record.EventType) {
                KEY_EVENT => {
                    // EventType identifies the initialized union member.
                    let key = unsafe { record.Event.KeyEvent };
                    if key.bKeyDown == 0 {
                        continue;
                    }
                    let modifiers = modifiers(key.dwControlKeyState);
                    let code = match key.wVirtualKeyCode {
                        VK_LEFT => Some(KeyCode::Left),
                        VK_RIGHT => Some(KeyCode::Right),
                        VK_UP => Some(KeyCode::Up),
                        VK_DOWN => Some(KeyCode::Down),
                        VK_HOME => Some(KeyCode::Home),
                        VK_END => Some(KeyCode::End),
                        VK_DELETE => Some(KeyCode::Delete),
                        VK_F2 => Some(KeyCode::F(2)),
                        _ => None,
                    };
                    if let Some(code) = code {
                        for _ in 0..key.wRepeatCount.max(1) {
                            self.decoder.key(code, modifiers);
                        }
                    } else {
                        let unit = unsafe { key.uChar.UnicodeChar };
                        if unit != 0 || key.wVirtualKeyCode == 0 {
                            self.decoder.utf16(unit, key.wRepeatCount, modifiers)?;
                        }
                    }
                }
                WINDOW_BUFFER_SIZE_EVENT => return Ok(Some(Input::Redraw)),
                FOCUS_EVENT => {
                    return Ok(Some(if unsafe { record.Event.FocusEvent.bSetFocus } == 0 {
                        Input::Hide
                    } else {
                        Input::Redraw
                    }))
                }
                _ => {}
            }
        }
    }

    pub fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        let text = std::str::from_utf8(bytes).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "terminal output is not UTF-8")
        })?;
        // WriteConsoleW preserves Unicode independently of the console code page.
        let wide = Zeroizing::new(text.encode_utf16().collect::<Vec<_>>());
        let mut offset = 0;
        while offset < wide.len() {
            let length = (wide.len() - offset).min(8192) as u32;
            let mut written = 0;
            if unsafe {
                WriteConsoleW(
                    handle(&self.output),
                    wide[offset..].as_ptr(),
                    length,
                    &mut written,
                    std::ptr::null(),
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            if written == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "console output closed",
                ));
            }
            offset += written as usize;
        }
        Ok(())
    }

    pub fn size(&self) -> io::Result<(u16, u16)> {
        let mut info = CONSOLE_SCREEN_BUFFER_INFO::default();
        if unsafe { GetConsoleScreenBufferInfo(handle(&self.output), &mut info) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((
            (info.srWindow.Right - info.srWindow.Left + 1) as u16,
            (info.srWindow.Bottom - info.srWindow.Top + 1) as u16,
        ))
    }

    pub fn restore(&mut self) -> io::Result<()> {
        let input = if self.restore_input {
            set_mode(&self.input, self.input_mode)
        } else {
            Ok(())
        };
        if input.is_ok() {
            self.restore_input = false;
        }
        let output = if self.restore_output {
            set_mode(&self.output, self.output_mode)
        } else {
            Ok(())
        };
        if output.is_ok() {
            self.restore_output = false;
        }
        input.and(output)
    }
}

impl Drop for Console {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

fn handle(file: &File) -> HANDLE {
    file.as_raw_handle() as HANDLE
}

fn mode(file: &File) -> io::Result<u32> {
    let mut value = 0;
    if unsafe { GetConsoleMode(handle(file), &mut value) } == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(value)
    }
}

fn set_mode(file: &File, value: u32) -> io::Result<()> {
    if unsafe { SetConsoleMode(handle(file), value) } == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn modifiers(state: u32) -> KeyModifiers {
    let mut result = KeyModifiers::NONE;
    if state & SHIFT_PRESSED != 0 {
        result |= KeyModifiers::SHIFT;
    }
    if state & (LEFT_ALT_PRESSED | RIGHT_ALT_PRESSED) != 0 {
        result |= KeyModifiers::ALT;
    }
    if state & (LEFT_CTRL_PRESSED | RIGHT_CTRL_PRESSED) != 0 {
        result |= KeyModifiers::CONTROL;
    }
    result
}
