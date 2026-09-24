//! Windows focused-input delivery. UI Automation runs on a dedicated MTA
//! thread which handles metadata only and never receives a credential. The
//! one value-bearing SendInput call stays on the owning operation: dropping
//! an expired/cancelled future cannot leave a delayed secret-typing task.

use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;
use tokio::sync::oneshot;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationValuePattern,
    UIA_EditControlTypeId, UIA_ValuePatternId,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetGUIThreadInfo, GetWindowThreadProcessId, GUITHREADINFO,
};

use super::{Backend, Deadline, Delivery, Effect, FillError, Reason};

fn fail(reason: Reason) -> FillError {
    FillError::new(reason, match reason {
        Reason::TargetChanged => "the foreground window or focused input changed; focus the intended empty field and run again",
        Reason::TargetReadonly => "the focused text input is read-only",
        Reason::TargetDisabled => "the focused text input is disabled",
        Reason::TargetHidden => "the focused text input is offscreen",
        Reason::TargetUnfillable => "focus a writable UI Automation Edit control and release modifier keys",
        Reason::Permission => "the desktop target could not be inspected; check the interactive session and application permissions",
        Reason::CleanupUnconfirmed => "the metadata-only accessibility worker did not stop within the cleanup deadline",
        _ => "Windows desktop input is unavailable in this session",
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Focus {
    window: usize,
    control: usize,
    thread: u32,
}

fn focus(pid: u32) -> Result<Focus, FillError> {
    // These are metadata queries only. Never read window text or a UIA value.
    unsafe {
        let window = GetForegroundWindow();
        if window.0.is_null() {
            return Err(fail(Reason::Permission));
        }
        let mut actual = 0;
        let thread = GetWindowThreadProcessId(window, Some(&mut actual));
        if pid == 0 || actual != pid || thread == 0 {
            return Err(fail(Reason::TargetChanged));
        }
        let mut info = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        GetGUIThreadInfo(thread, &mut info).map_err(|_| fail(Reason::Permission))?;
        if info.hwndFocus.0.is_null() {
            return Err(fail(Reason::TargetUnfillable));
        }
        if [VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN]
            .iter()
            .any(|key| GetAsyncKeyState(key.0 as i32) < 0)
        {
            return Err(fail(Reason::TargetUnfillable));
        }
        Ok(Focus {
            window: window.0 as usize,
            control: info.hwndFocus.0 as usize,
            thread,
        })
    }
}

enum Request {
    Inspect(oneshot::Sender<Result<Focus, FillError>>),
}

struct Inspector {
    sender: Option<mpsc::Sender<Request>>,
    worker: Option<JoinHandle<()>>,
}
impl Inspector {
    fn start(pid: u32) -> Result<Self, FillError> {
        let (sender, receiver) = mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("agentenv-uia-metadata".into())
            .spawn(move || {
                // Microsoft requires non-window-owning MTA threads for UIA calls.
                let initialized = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
                let automation: Result<IUIAutomation, _> = if initialized {
                    unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }
                } else {
                    Err(windows::core::Error::from_hresult(windows::core::HRESULT(
                        0x80070005u32 as i32,
                    )))
                };
                let mut prepared = None;
                while let Ok(Request::Inspect(reply)) = receiver.recv() {
                    // A dropped waiter revokes the work. Even an in-flight UIA
                    // call can only return metadata and cannot perform insertion.
                    if reply.is_closed() {
                        continue;
                    }
                    let result = match &automation {
                        Ok(automation) => inspect(automation, pid, &mut prepared),
                        Err(_) => Err(fail(Reason::Permission)),
                    };
                    let _ = reply.send(result);
                }
                drop(prepared);
                drop(automation);
                if initialized {
                    unsafe {
                        CoUninitialize();
                    }
                }
            })
            .map_err(|_| fail(Reason::BackendUnavailable))?;
        Ok(Self {
            sender: Some(sender),
            worker: Some(worker),
        })
    }
    async fn inspect(&self) -> Result<Focus, FillError> {
        let (reply, response) = oneshot::channel();
        self.sender
            .as_ref()
            .ok_or_else(|| fail(Reason::BackendUnavailable))?
            .send(Request::Inspect(reply))
            .map_err(|_| fail(Reason::BackendUnavailable))?;
        response
            .await
            .map_err(|_| fail(Reason::BackendUnavailable))?
    }
    async fn stop(&mut self, grace: &Deadline) -> Result<(), FillError> {
        self.sender.take();
        while self
            .worker
            .as_ref()
            .is_some_and(|worker| !worker.is_finished())
        {
            if grace.expired() {
                return Err(fail(Reason::CleanupUnconfirmed));
            }
            tokio::time::sleep(Duration::from_millis(10).min(grace.remaining())).await;
        }
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| fail(Reason::CleanupUnconfirmed))?;
        }
        Ok(())
    }
}

fn inspect(
    automation: &IUIAutomation,
    pid: u32,
    prepared: &mut Option<(IUIAutomationElement, Focus)>,
) -> Result<Focus, FillError> {
    let before = focus(pid)?;
    unsafe {
        let element = automation
            .GetFocusedElement()
            .map_err(|_| fail(Reason::Permission))?;
        if element
            .CurrentProcessId()
            .map_err(|_| fail(Reason::Permission))?
            != pid as i32
            || !element
                .CurrentHasKeyboardFocus()
                .map_err(|_| fail(Reason::Permission))?
                .as_bool()
        {
            return Err(fail(Reason::TargetChanged));
        }
        if element
            .CurrentControlType()
            .map_err(|_| fail(Reason::Permission))?
            != UIA_EditControlTypeId
        {
            return Err(fail(Reason::TargetUnfillable));
        }
        if !element
            .CurrentIsEnabled()
            .map_err(|_| fail(Reason::Permission))?
            .as_bool()
        {
            return Err(fail(Reason::TargetDisabled));
        }
        if element
            .CurrentIsOffscreen()
            .map_err(|_| fail(Reason::Permission))?
            .as_bool()
        {
            return Err(fail(Reason::TargetHidden));
        }
        // Inspect read-only metadata, NOT CurrentValue. Controls which cannot
        // prove editability are refused, including inaccessible custom widgets.
        let pattern: IUIAutomationValuePattern = element
            .GetCurrentPatternAs(UIA_ValuePatternId)
            .map_err(|_| fail(Reason::TargetUnfillable))?;
        if pattern
            .CurrentIsReadOnly()
            .map_err(|_| fail(Reason::Permission))?
            .as_bool()
        {
            return Err(fail(Reason::TargetReadonly));
        }
        if let Some((old, old_focus)) = prepared.as_ref() {
            if *old_focus != before
                || !automation
                    .CompareElements(old, &element)
                    .map_err(|_| fail(Reason::TargetChanged))?
                    .as_bool()
            {
                return Err(fail(Reason::TargetChanged));
            }
        }
        if focus(pid)? != before {
            return Err(fail(Reason::TargetChanged));
        }
        if prepared.is_none() {
            *prepared = Some((element, before));
        }
    }
    Ok(before)
}

/// A zeroized, exactly allocated SendInput batch. No clipboard, key-name
/// interpretation, layout conversion, select-all or Enter is involved.
struct InputBatch(Vec<INPUT>);
impl InputBatch {
    fn new(value: &str) -> Self {
        let count = value.encode_utf16().count() * 2;
        let mut inputs = Vec::with_capacity(count);
        for unit in value.encode_utf16() {
            for flags in [KEYEVENTF_UNICODE, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP] {
                inputs.push(INPUT {
                    r#type: INPUT_KEYBOARD,
                    Anonymous: INPUT_0 {
                        ki: KEYBDINPUT {
                            wScan: unit,
                            dwFlags: flags,
                            ..Default::default()
                        },
                    },
                });
            }
        }
        Self(inputs)
    }
}
impl Drop for InputBatch {
    fn drop(&mut self) {
        // INPUT is a C union containing only plain scalar fields. Volatile
        // writes cover initialized storage and padding without reading it.
        let length = self.0.len() * std::mem::size_of::<INPUT>();
        let bytes = self.0.as_mut_ptr().cast::<u8>();
        for index in 0..length {
            unsafe {
                bytes.add(index).write_volatile(0);
            }
        }
        std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
    }
}

pub struct DesktopBackend {
    pid: u32,
    inspector: Option<Inspector>,
}
impl DesktopBackend {
    pub fn new(expected_pid: u32) -> Self {
        Self {
            pid: expected_pid,
            inspector: None,
        }
    }
}
impl Backend for DesktopBackend {
    fn name(&self) -> &'static str {
        "desktop"
    }
    async fn prepare(&mut self, _deadline: &Deadline) -> Result<(), FillError> {
        focus(self.pid)?;
        self.inspector = Some(Inspector::start(self.pid)?);
        self.inspector
            .as_ref()
            .ok_or_else(|| fail(Reason::BackendUnavailable))?
            .inspect()
            .await?;
        Ok(())
    }
    async fn deliver(&mut self, delivery: &Delivery) -> Result<Effect, FillError> {
        let inspected = self
            .inspector
            .as_ref()
            .ok_or_else(|| fail(Reason::BackendUnavailable))?
            .inspect()
            .await?;
        if focus(self.pid)? != inspected {
            return Err(fail(Reason::TargetChanged));
        }
        let batch = InputBatch::new(delivery.begin_mutation()?.as_str());
        // No await or delegated task after the mutation gate. Recheck native
        // focus and time after encoding; any failure now is conservatively
        // uncertain, as required by the coordinator's single-send contract.
        if focus(self.pid)? != inspected {
            return Err(fail(Reason::TargetChanged));
        }
        if delivery.deadline().expired() {
            return Err(FillError::new(
                Reason::Timeout,
                "the input deadline expired",
            ));
        }
        let sent = unsafe { SendInput(&batch.0, std::mem::size_of::<INPUT>() as i32) };
        if sent as usize != batch.0.len() {
            return Err(FillError::uncertain(Reason::DeliveryFailed, "Windows did not accept the entire input batch; check target integrity and inspect the field before retrying"));
        }
        Ok(Effect::InputSent)
    }
    async fn release(&mut self, grace: &Deadline) -> Result<(), FillError> {
        if let Some(inspector) = &mut self.inspector {
            inspector.stop(grace).await?;
        }
        self.inspector.take();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_is_a_literal_balanced_input_batch() {
        let batch = InputBatch::new(" 密码🔑 ");
        let units: Vec<u16> = " 密码🔑 ".encode_utf16().collect();
        assert_eq!(batch.0.len(), units.len() * 2);
        for (pair, unit) in batch.0.as_chunks::<2>().0.iter().zip(units) {
            for input in pair {
                assert_eq!(input.r#type, INPUT_KEYBOARD);
            }
            unsafe {
                assert_eq!(pair[0].Anonymous.ki.wScan, unit);
                assert_eq!(pair[1].Anonymous.ki.wScan, unit);
                assert_eq!(pair[0].Anonymous.ki.wVk.0, 0);
                assert_eq!(pair[0].Anonymous.ki.dwFlags, KEYEVENTF_UNICODE);
                assert_eq!(
                    pair[1].Anonymous.ki.dwFlags,
                    KEYEVENTF_UNICODE | KEYEVENTF_KEYUP
                );
            }
        }
    }
}
