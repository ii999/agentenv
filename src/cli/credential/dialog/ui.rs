use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use eframe::egui::{self, Event, ImeEvent, Key, Modifiers, RichText, TextBuffer};
use zeroize::Zeroize;

use super::{
    input::display_text,
    platform,
    session::Session,
    theme::{self, GlassButton, Palette, RevealToggle},
};

const INPUT_ID: &str = "credential-value";

/// Window margin at the sides and bottom, in points.
const EDGE: i8 = 20;
/// Top margin under the transparent macOS title bar and its window buttons.
const GLASS_TOP: i8 = 36;
/// Vertical gap between the header, the field and the buttons.
const SECTION_GAP: f32 = 16.0;
/// Gap above a status message under the field.
const STATUS_GAP: f32 = 10.0;
/// Horizontal gap between the footer buttons.
const BUTTON_GAP: f32 = 10.0;
/// Width of each footer button; the pair is centered.
const BUTTON_WIDTH: f32 = 110.0;
/// Inset of the reveal toggle from the field's bottom-right corner.
const TOGGLE_INSET: f32 = 6.0;

pub(super) struct Dialog {
    session: Rc<RefCell<Session>>,
    cancellation: agentenv::cancel::Cancellation,
    clipboard: Option<platform::Clipboard>,
    paste_requested: bool,
    composing: bool,
    feedback: Option<&'static str>,
    /// A system material shows through the window behind the content.
    glass: bool,
    /// Height of the status messages below the field in the previous frame;
    /// the field fills the rest of the window body.
    status_height: f32,
}

impl Dialog {
    pub fn new(
        creation: &eframe::CreationContext<'_>,
        session: Rc<RefCell<Session>>,
        cancellation: agentenv::cancel::Cancellation,
    ) -> std::io::Result<Self> {
        let glass = platform::install_backdrop(creation);
        theme::install(&creation.egui_ctx, glass);
        Ok(Self {
            session,
            cancellation,
            clipboard: Some(platform::Clipboard::new(creation)?),
            paste_requested: false,
            composing: false,
            feedback: None,
            glass,
            status_height: 0.0,
        })
    }

    fn paste(&mut self, primary: bool) -> Option<String> {
        let value = self.clipboard.as_mut().and_then(|clipboard| {
            if primary {
                clipboard.primary()
            } else {
                clipboard.text()
            }
        });
        self.feedback = value
            .is_none()
            .then_some("Nothing was pasted. The clipboard has no text or could not be read.");
        value
    }

    fn content(&mut self, ui: &mut egui::Ui, body_height: f32) {
        let context = ui.ctx().clone();
        let palette = Palette::of(ui, self.glass);
        let mut session = self.session.borrow_mut();
        let busy = session.saving();
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            theme::emblem(ui, palette);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 3.0;
                ui.add_space(2.0);
                ui.label(
                    RichText::new("Save credential")
                        .heading()
                        .color(palette.text),
                );
                ui.add(
                    egui::Label::new(
                        RichText::new(&session.definition.name).color(palette.secondary),
                    )
                    .wrap(),
                );
            });
        });
        ui.add_space(SECTION_GAP);
        let revealed = session.revealed;
        let mut layouter = |ui: &egui::Ui, buffer: &dyn TextBuffer, width: f32| {
            let text = display_text(buffer.as_str(), revealed);
            ui.fonts_mut(|fonts| {
                fonts.layout(
                    text,
                    egui::FontId::monospace(14.0),
                    ui.visuals().text_color(),
                    width,
                )
            })
        };
        // The field fills the body left by the header and status messages,
        // keeping room for its focus ring inside the scroll area's clip.
        let used = ui.cursor().top() - ui.max_rect().top();
        let field_height =
            (body_height - used - self.status_height - theme::FOCUS_RING_OUTSET).max(0.0);
        // The definition's description tells the person what to enter.
        let hint = session.definition.description.clone();
        let editor = ui.add_enabled(
            !busy,
            egui::TextEdit::multiline(&mut session.draft)
                .id(egui::Id::unique(INPUT_ID))
                .hint_text(hint)
                .password(true)
                .desired_rows(4)
                .min_size(egui::vec2(0.0, field_height))
                .desired_width(f32::INFINITY)
                // The bottom margin keeps text clear of the reveal toggle.
                .margin(egui::Margin {
                    left: 12,
                    right: 12,
                    top: 10,
                    bottom: (theme::REVEAL_TOGGLE_SIZE + TOGGLE_INSET) as i8,
                })
                .lock_focus(false)
                .layouter(&mut layouter),
        );
        if editor.has_focus() {
            theme::focus_ring(ui.painter(), editor.rect, theme::FIELD_RADIUS, palette);
        }
        let toggle_rect = egui::Rect::from_min_size(
            editor.rect.right_bottom()
                - egui::Vec2::splat(TOGGLE_INSET + theme::REVEAL_TOGGLE_SIZE),
            egui::Vec2::splat(theme::REVEAL_TOGGLE_SIZE),
        );
        let toggle = ui
            .add_enabled_ui(!busy, |ui| {
                ui.put(toggle_rect, RevealToggle::new(session.revealed, self.glass))
            })
            .inner;
        if toggle.clicked() {
            session.revealed = !session.revealed;
            session.focus_input = true;
        }
        if editor.changed() {
            session.clear_error();
            self.feedback = None;
        }
        context.accesskit_node_builder(editor.id, |node| node.set_label("Credential value"));
        redact_accessibility(&context, editor.id);
        if session.focus_input && !busy {
            editor.request_focus();
            session.focus_input = false;
        }
        editor.context_menu(|ui| {
            if ui.add_enabled(!busy, egui::Button::new("Paste")).clicked() {
                self.paste_requested = true;
                session.focus_input = true;
                context.request_repaint();
                ui.close();
            }
            if ui.add_enabled(!busy, egui::Button::new("Clear")).clicked() {
                session.draft.clear();
                session.clear_error();
                session.focus_input = true;
                self.feedback = None;
                ui.close();
            }
        });
        let status_top = ui.cursor().top();
        if let Some(error) = &session.error {
            ui.add_space(STATUS_GAP);
            egui::Frame::new()
                .fill(palette.danger.gamma_multiply(0.12))
                .stroke(egui::Stroke::new(1.0, palette.danger.gamma_multiply(0.4)))
                .corner_radius(theme::FIELD_RADIUS)
                .inner_margin(egui::Margin::symmetric(12, 10))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing.y = 6.0;
                    ui.add(egui::Label::new(RichText::new(error).color(palette.danger)).wrap());
                    if let Some(details) = &session.error_details {
                        egui::CollapsingHeader::new(
                            RichText::new("Technical details")
                                .small()
                                .color(palette.secondary),
                        )
                        .show(ui, |ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(details).small().color(palette.secondary),
                                )
                                .wrap(),
                            );
                        });
                    }
                });
        } else if let Some(feedback) = self.feedback {
            ui.add_space(STATUS_GAP);
            ui.label(RichText::new(feedback).small().color(palette.secondary));
        }
        if busy {
            ui.add_space(STATUS_GAP);
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(14.0).color(palette.accent));
                ui.label(
                    RichText::new(if session.closing() {
                        "Finishing the save before closing…"
                    } else {
                        "Saving…"
                    })
                    .color(palette.secondary),
                );
            });
        }
        let status_height = ui.cursor().top() - status_top;
        if (status_height - self.status_height).abs() > 0.5 {
            self.status_height = status_height;
            context.request_repaint();
        }
    }

    fn draw(&mut self, ui: &mut egui::Ui) {
        // Consume the save shortcut before TextEdit handles Enter; an IME
        // commit in this frame still reaches the editor before the save below.
        let save_key = !self.composing
            && ui.input_mut(|input| {
                input.consume_shortcut(&egui::KeyboardShortcut::new(Modifiers::COMMAND, Key::Enter))
            });
        let cancel_key = !self.composing
            && ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape));
        let palette = Palette::of(ui, self.glass);
        let glass = self.glass;
        // The tint follows the backdrop's rounded corners; an opaque window
        // fills its whole rectangle.
        let window_radius = if glass {
            theme::WINDOW_CORNER_RADIUS
        } else {
            0
        };
        ui.painter()
            .rect_filled(ui.max_rect(), window_radius, palette.surface);
        let margin = egui::Margin {
            left: EDGE,
            right: EDGE,
            top: if glass { GLASS_TOP } else { EDGE },
            bottom: EDGE,
        };
        egui::Frame::new().inner_margin(margin).show(ui, |ui| {
            // Every vertical gap below is explicit, so the body and the
            // button row add up to the available height exactly.
            ui.spacing_mut().item_spacing.y = 0.0;
            // The body ends with the field's focus-ring room, which also
            // counts toward the gap above the buttons.
            let footer_gap = SECTION_GAP - theme::FOCUS_RING_OUTSET;
            let body_height = (ui.available_height() - footer_gap - theme::BUTTON_HEIGHT).max(0.0);
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .max_height(body_height)
                .show(ui, |ui| self.content(ui, body_height));
            ui.add_space(footer_gap);
            let mut session = self.session.borrow_mut();
            let busy = session.saving();
            let mut save = false;
            let mut cancel = cancel_key;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = BUTTON_GAP;
                ui.add_space(
                    ((ui.available_width() - 2.0 * BUTTON_WIDTH - BUTTON_GAP) / 2.0).max(0.0),
                );
                cancel |= ui
                    .add(
                        GlassButton::secondary(if busy { "Close" } else { "Cancel" }, glass)
                            .min_width(BUTTON_WIDTH),
                    )
                    .clicked();
                save = ui
                    .add_enabled(
                        !busy && !self.composing,
                        GlassButton::primary("Save", glass).min_width(BUTTON_WIDTH),
                    )
                    .clicked()
                    || save_key;
            });
            if cancel {
                session.cancel(2);
            } else if save {
                session.save(ui.ctx());
            }
        });
    }
}

impl eframe::App for Dialog {
    fn logic(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        let mut session = self.session.borrow_mut();
        session.poll();
        if let Some(signal) = self.cancellation.pending() {
            session.cancel(signal);
        }
        if context.input(|input| input.viewport().focused == Some(false)) {
            session.revealed = false;
        }
        if context.input(|input| input.viewport().close_requested()) {
            session.cancel(2);
            if session.saving() {
                context.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            }
        }
        if session.done() {
            context.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        // Signal delivery must still be observed when the window is idle or
        // hidden; a store completion also requests an immediate repaint.
        context.request_repaint_after(Duration::from_millis(100));
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        if self.glass {
            [0.0; 4]
        } else {
            visuals.panel_fill.to_normalized_gamma_f32()
        }
    }

    fn raw_input_hook(&mut self, context: &egui::Context, input: &mut egui::RawInput) {
        let editing = !self.session.borrow().saving()
            && (self.paste_requested
                || context.memory(|memory| memory.has_focus(egui::Id::unique(INPUT_ID))));
        let mut events = Vec::with_capacity(input.events.len() + usize::from(self.paste_requested));
        if self.paste_requested {
            self.paste_requested = false;
            if let Some(text) = self.paste(false) {
                events.push(Event::Paste(text));
            }
            context.memory_mut(|memory| memory.request_focus(egui::Id::unique(INPUT_ID)));
        }
        for event in input.events.drain(..) {
            match &event {
                Event::Ime(ImeEvent::Preedit { text, .. }) => self.composing = !text.is_empty(),
                Event::Ime(ImeEvent::Commit(_)) => self.composing = false,
                _ => {}
            }
            if editing {
                if let Some(event) = clipboard_event(event, &mut |primary| self.paste(primary)) {
                    events.push(event);
                }
            } else {
                events.push(event);
            }
        }
        input.events = events;
    }
}

/// TextEdit derives accessibility text runs from its visual layout. Keep
/// those runs masked even when a human explicitly reveals the local preview.
/// Labels, editing actions, selection indices and cursor geometry remain.
fn redact_accessibility(context: &egui::Context, editor: egui::Id) {
    let mut pending = vec![editor];
    while let Some(id) = pending.pop() {
        if let Some(children) = context.accesskit_node_builder(id, |node| {
            if let Some(value) = node.value() {
                let masked = display_text(value, false);
                if node.role() == egui::accesskit::Role::TextRun {
                    node.set_character_lengths(
                        masked
                            .chars()
                            .map(|character| character.len_utf8() as u8)
                            .collect::<Vec<_>>(),
                    );
                    node.clear_word_starts();
                }
                node.set_value(masked);
            }
            node.children().to_vec()
        }) {
            pending.extend(children.into_iter().map(|id| {
                // These bits came from egui's own Id::accesskit_id mapping,
                // so they already have the entropy this constructor requires.
                unsafe { egui::Id::from_high_entropy_bits(id.0) }
            }));
        }
    }
}

fn paste_key(key: Key, modifiers: Modifiers) -> bool {
    key == Key::Paste
        || (key == Key::V && modifiers.command_only())
        || (cfg!(windows) && key == Key::Insert && modifiers.shift && !modifiers.ctrl)
}
fn copy_key(key: Key, modifiers: Modifiers) -> bool {
    key == Key::Copy
        || (key == Key::C && modifiers.command_only())
        || (cfg!(windows) && key == Key::Insert && modifiers.ctrl)
}
fn cut_key(key: Key, modifiers: Modifiers) -> bool {
    key == Key::Cut
        || (key == Key::X && modifiers.command_only())
        || (cfg!(windows) && key == Key::Delete && modifiers.shift)
}

/// Read raw clipboard text only for explicit paste actions. The integration's
/// keyboard and context-menu conversion otherwise replaces CRLF with LF.
fn clipboard_event(
    mut event: Event,
    read: &mut impl FnMut(bool) -> Option<String>,
) -> Option<Event> {
    match &mut event {
        Event::Paste(text) => {
            text.zeroize();
            read(false).map(Event::Paste)
        }
        Event::MiddleClickPaste { text, pos } => {
            text.zeroize();
            read(true).map(|text| Event::MiddleClickPaste { text, pos: *pos })
        }
        Event::Key {
            key,
            modifiers,
            pressed: true,
            ..
        } if paste_key(*key, *modifiers) => read(false).map(Event::Paste),
        Event::Key {
            key,
            modifiers,
            pressed: true,
            ..
        } if copy_key(*key, *modifiers) => Some(Event::Copy),
        Event::Key {
            key,
            modifiers,
            pressed: true,
            ..
        } if cut_key(*key, *modifiers) => Some(Event::Cut),
        _ => Some(event),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentenv::config::{CredentialDef, CredentialUsage, Provider};

    const CANARY: &str = "dialog-canary-7mQ9";

    fn fixture() -> (Dialog, egui::Context) {
        let session = Rc::new(RefCell::new(Session::new(CredentialDef {
            name: "render_fixture".into(),
            description: "Dialog rendering fixture.".into(),
            provider: Provider::Env {
                name: "UNUSED_RENDER_FIXTURE".into(),
            },
            inject_as: Some("FIXTURE_VALUE".into()),
            usages: vec![CredentialUsage::Environment],
        })));
        let cancellation = agentenv::cancel::Cancellation::never();
        let dialog = Dialog {
            session,
            cancellation,
            clipboard: None,
            paste_requested: false,
            composing: false,
            feedback: None,
            glass: false,
            status_height: 0.0,
        };
        let context = egui::Context::default();
        theme::install(&context, false);
        context.enable_accesskit();
        (dialog, context)
    }

    fn render(
        dialog: &mut Dialog,
        context: &egui::Context,
        events: Vec<Event>,
    ) -> egui::FullOutput {
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(520.0, 520.0),
                )),
                focused: true,
                events,
                ..Default::default()
            },
            |ui| dialog.draw(ui),
        );
        output.textures_delta.clear();
        output
    }

    fn key(key: Key, modifiers: Modifiers) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    #[test]
    fn masking_and_accessibility_never_publish_the_candidate() {
        let (mut dialog, context) = fixture();
        dialog.session.borrow_mut().draft.replace_with(CANARY);
        for revealed in [false, true, false] {
            dialog.session.borrow_mut().revealed = revealed;
            let output = render(&mut dialog, &context, Vec::new());
            let accessibility = output
                .platform_output
                .accesskit_update
                .as_ref()
                .expect("accessibility is enabled");
            assert!(!format!("{accessibility:?}").contains(CANARY));
            assert!(accessibility
                .nodes
                .iter()
                .any(|(_, node)| node.role() == egui::accesskit::Role::PasswordInput));
            assert_eq!(format!("{:?}", output.shapes).contains(CANARY), revealed);
            assert!(!format!("{:?}", output.platform_output.events).contains(CANARY));
            assert!(!format!("{:?}", output.platform_output.commands).contains(CANARY));
        }
    }

    #[test]
    fn widget_paste_preserves_crlf_unicode_and_whitespace_and_blocks_copy_export() {
        let (mut dialog, context) = fixture();
        let _ = render(&mut dialog, &context, Vec::new());
        let value = "  dialog-canary-7mQ9-密钥-🙂-e\u{301}\r\nsecond\tline\n";
        let _ = render(&mut dialog, &context, vec![Event::Paste(value.into())]);
        assert_eq!(dialog.session.borrow().draft.as_str(), value);
        let output = render(
            &mut dialog,
            &context,
            vec![key(Key::A, Modifiers::COMMAND), Event::Copy],
        );
        assert!(!output
            .platform_output
            .commands
            .iter()
            .any(|command| matches!(command, egui::OutputCommand::CopyText(_))));
        let output = render(&mut dialog, &context, vec![Event::Cut]);
        assert!(dialog.session.borrow().draft.as_str().is_empty());
        assert!(!output
            .platform_output
            .commands
            .iter()
            .any(|command| matches!(command, egui::OutputCommand::CopyText(_))));
    }

    #[test]
    fn exact_clipboard_path_replaces_normalized_events_and_does_not_read_for_typing() {
        let raw = "  line-one\r\n密钥\t\r\n";
        let mut reads = 0;
        let event = clipboard_event(Event::Paste(raw.replace("\r\n", "\n")), &mut |primary| {
            assert!(!primary);
            reads += 1;
            Some(raw.into())
        });
        assert!(matches!(event, Some(Event::Paste(text)) if text == raw));
        assert_eq!(reads, 1);
        let event = clipboard_event(key(Key::V, Modifiers::COMMAND), &mut |_| Some(raw.into()));
        assert!(matches!(event, Some(Event::Paste(text)) if text == raw));
        let _ = clipboard_event(Event::Text("typed".into()), &mut |_| {
            panic!("typing must not read the clipboard")
        });
        let _ = clipboard_event(
            key(
                Key::V,
                Modifiers {
                    ctrl: true,
                    alt: true,
                    command: true,
                    ..Default::default()
                },
            ),
            &mut |_| panic!("AltGr input must not read the clipboard"),
        );
        assert!(clipboard_event(key(Key::V, Modifiers::COMMAND), &mut |_| None).is_none());
    }

    #[test]
    fn save_shortcut_does_not_insert_a_newline_and_active_composition_prevents_save() {
        let (mut dialog, context) = fixture();
        let _ = render(&mut dialog, &context, Vec::new());
        dialog.session.borrow_mut().draft.replace_with(CANARY);
        dialog.composing = true;
        let _ = render(
            &mut dialog,
            &context,
            vec![key(Key::Enter, Modifiers::COMMAND)],
        );
        assert!(!dialog.session.borrow().saving());
        dialog.composing = false;
        let _ = render(
            &mut dialog,
            &context,
            vec![key(Key::Enter, Modifiers::COMMAND)],
        );
        assert!(dialog.session.borrow().saving());
        assert_eq!(dialog.session.borrow().draft.as_str(), CANARY);
        // The fixture provider refuses storage without touching a keychain.
        let completion = dialog.session.borrow_mut().finish(false);
        assert_ne!(completion, super::super::Completion::Stored);
        let output = completion.output("render_fixture", true);
        assert!(!output.stdout.contains(CANARY));
        assert!(!output.stderr.contains(CANARY));
    }
}
