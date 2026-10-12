mod editor;
mod render;

use std::error::Error;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use editor::{Editor, SelectionUnit};
use render::{MARGIN, Renderer};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, Ime, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};
use winkin::selection::{Granularity, MotionDirection};

const SAMPLE: &str = "Winkin text editor\n\nType here. Select with the mouse or Shift + arrows. Resize the window to reflow.\n\nUnicode: café, e\u{301}, Ελληνικά, 日本語, 中文, 한국어, 👩‍👩‍👧‍👦.\nMixed directions: English العربية English עברית 123.\nעברית: טקסט מימין לשמאל\n\nTabs and spaces are preserved:\n\tfirst\n\tsecond     aligned\n\nTry your input method, paste a paragraph, or delete everything and start again.\n";
const BLINK: Duration = Duration::from_millis(550);

type Surface = softbuffer::Surface<Rc<Window>, Rc<Window>>;

struct App {
    editor: Editor,
    renderer: Renderer,
    window: Option<Rc<Window>>,
    surface: Option<Surface>,
    modifiers: ModifiersState,
    pointer: PhysicalPosition<f64>,
    dragging: bool,
    last_click: Option<(Instant, PhysicalPosition<f64>)>,
    click_count: u8,
    scroll: f32,
    clipboard: Option<arboard::Clipboard>,
    file: Option<PathBuf>,
    focused: bool,
    blink: Instant,
    caret: bool,
}

impl App {
    fn new(text: String, file: Option<PathBuf>) -> Self {
        Self {
            editor: Editor::new(text),
            renderer: Renderer::new(),
            window: None,
            surface: None,
            modifiers: ModifiersState::empty(),
            pointer: PhysicalPosition::new(0.0, 0.0),
            dragging: false,
            last_click: None,
            click_count: 0,
            scroll: 0.0,
            clipboard: arboard::Clipboard::new().ok(),
            file,
            focused: true,
            blink: Instant::now(),
            caret: true,
        }
    }

    fn viewport(&self) -> (f32, f32) {
        let window = self.window.as_ref().unwrap();
        let size = window.inner_size().to_logical::<f32>(window.scale_factor());
        (size.width, size.height)
    }

    fn clamp_scroll(&mut self) {
        let (_, height) = self.viewport();
        self.scroll = self.scroll.clamp(
            0.0,
            (self.editor.layout.metrics().block_end + 2.0 * MARGIN - height).max(0.0),
        );
    }

    fn changed(&mut self) {
        self.caret = true;
        self.blink = Instant::now();
        if let Some(caret) = self.editor.layout.caret(self.editor.selection.focus()) {
            let line = self.editor.layout.line(caret.line).unwrap().metrics();
            let (_, height) = self.viewport();
            let top = line.top + caret.block.over;
            let bottom = line.top + caret.block.under;
            self.scroll = self.scroll.min(top).max(bottom - height + 2.0 * MARGIN);
            self.clamp_scroll();
        }
        self.ime_cursor();
        let window = self.window.as_ref().unwrap();
        window.set_title(&format!(
            "Winkin — text editor — {}",
            self.editor.focus_description()
        ));
        window.request_redraw();
    }

    fn ime_cursor(&self) {
        if let Some(caret) = self.editor.layout.caret(self.editor.selection.focus()) {
            let line = self.editor.layout.line(caret.line).unwrap().metrics();
            let window = self.window.as_ref().unwrap();
            let scale = window.scale_factor();
            window.set_ime_cursor_area(
                PhysicalPosition::new(
                    f64::from(MARGIN + line.left + caret.inline.left) * scale,
                    f64::from(MARGIN + line.top + caret.block.over - self.scroll) * scale,
                ),
                PhysicalSize::new(
                    scale,
                    f64::from(caret.block.under - caret.block.over) * scale,
                ),
            );
        }
    }

    fn pointer_selection(&mut self, dragging: bool) {
        let scale = self.window.as_ref().unwrap().scale_factor() as f32;
        let x = self.pointer.x as f32 / scale - MARGIN;
        let y = self.pointer.y as f32 / scale - MARGIN + self.scroll;
        if dragging {
            self.editor.drag_pointer(x, y);
        } else {
            let unit = match self.click_count {
                2 => SelectionUnit::Word,
                3 => SelectionUnit::Line,
                _ => SelectionUnit::Character,
            };
            self.editor
                .pointer_unit(x, y, unit, self.modifiers.shift_key());
        }
        self.changed();
    }

    fn key(&mut self, key: Key, text: Option<&str>) {
        self.last_click = None;
        let command = if cfg!(target_os = "macos") {
            self.modifiers.super_key()
        } else {
            self.modifiers.control_key()
        };
        let word = if cfg!(target_os = "macos") {
            self.modifiers.alt_key()
        } else {
            self.modifiers.control_key()
        };
        let shift = self.modifiers.shift_key();
        if command && let Key::Character(character) = &key {
            match character.to_lowercase().as_str() {
                "a" => self.editor.select_all(),
                "c" | "x" => {
                    self.editor.cancel_composition();
                    if !self.editor.selection.is_collapsed()
                        && let Some(clipboard) = &mut self.clipboard
                    {
                        match clipboard.set_text(self.editor.selected_text()) {
                            Ok(()) if character.eq_ignore_ascii_case("x") => self.editor.insert(""),
                            Err(error) => eprintln!("Clipboard: {error}"),
                            _ => {}
                        }
                    }
                }
                "v" => {
                    if let Some(clipboard) = &mut self.clipboard {
                        match clipboard.get_text() {
                            Ok(text) => self.editor.insert(&text),
                            Err(error) => eprintln!("Clipboard: {error}"),
                        }
                    }
                }
                "z" if shift => self.editor.redo(),
                "z" => self.editor.undo(),
                "y" => self.editor.redo(),
                "s" => {
                    self.editor.cancel_composition();
                    if let Some(path) = &self.file
                        && let Err(error) = std::fs::write(path, &self.editor.text)
                    {
                        eprintln!("Save: {error}");
                    }
                }
                _ => return,
            }
            self.changed();
            return;
        }
        // The input method owns editing keys while a preedit is active.
        if self.editor.composing() && key != Key::Named(NamedKey::Escape) {
            return;
        }
        let character = if word {
            Granularity::Word
        } else {
            Granularity::Character
        };
        let (direction, granularity) = match key {
            Key::Named(NamedKey::ArrowLeft) => (
                MotionDirection::Left,
                if command && cfg!(target_os = "macos") {
                    Granularity::LineBoundary
                } else {
                    character
                },
            ),
            Key::Named(NamedKey::ArrowRight) => (
                MotionDirection::Right,
                if command && cfg!(target_os = "macos") {
                    Granularity::LineBoundary
                } else {
                    character
                },
            ),
            Key::Named(NamedKey::ArrowUp) => (
                MotionDirection::Backward,
                if command {
                    Granularity::DocumentBoundary
                } else {
                    Granularity::Line
                },
            ),
            Key::Named(NamedKey::ArrowDown) => (
                MotionDirection::Forward,
                if command {
                    Granularity::DocumentBoundary
                } else {
                    Granularity::Line
                },
            ),
            // Home and End go to the line's left and right edges, as the
            // arrows go on the screen; with the command key, to the text's
            // start and end.
            Key::Named(NamedKey::Home) if command => {
                (MotionDirection::Backward, Granularity::DocumentBoundary)
            }
            Key::Named(NamedKey::End) if command => {
                (MotionDirection::Forward, Granularity::DocumentBoundary)
            }
            Key::Named(NamedKey::Home) => (MotionDirection::Left, Granularity::LineBoundary),
            Key::Named(NamedKey::End) => (MotionDirection::Right, Granularity::LineBoundary),
            Key::Named(NamedKey::Backspace) => {
                self.editor.delete(MotionDirection::Backward, word);
                self.changed();
                return;
            }
            Key::Named(NamedKey::Delete) => {
                self.editor.delete(MotionDirection::Forward, word);
                self.changed();
                return;
            }
            Key::Named(NamedKey::Enter) => {
                self.editor.insert("\n");
                self.changed();
                return;
            }
            Key::Named(NamedKey::Tab) => {
                self.editor.insert("\t");
                self.changed();
                return;
            }
            Key::Named(NamedKey::Escape) => {
                self.editor.cancel_composition();
                self.changed();
                return;
            }
            _ => {
                if !command
                    && !self.modifiers.control_key()
                    && let Some(text) = text.filter(|text| !text.chars().any(char::is_control))
                {
                    self.editor.insert(text);
                    self.changed();
                }
                return;
            }
        };
        self.editor.navigate(direction, granularity, shift);
        self.changed();
    }

    fn draw(&mut self) -> Result<(), Box<dyn Error>> {
        let window = self.window.as_ref().unwrap();
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            return Ok(());
        }
        let width = size.width.min(u32::from(u16::MAX)) as u16;
        let height = size.height.min(u32::from(u16::MAX)) as u16;
        let pixmap = self.renderer.draw(
            &self.editor,
            width,
            height,
            window.scale_factor(),
            self.scroll,
            self.caret && self.focused,
        );
        let surface = self.surface.as_mut().unwrap();
        surface.resize(
            NonZeroU32::new(u32::from(width)).unwrap(),
            NonZeroU32::new(u32::from(height)).unwrap(),
        )?;
        let mut buffer = surface.buffer_mut()?;
        for (target, pixel) in buffer.iter_mut().zip(pixmap.data()) {
            *target = (u32::from(pixel.r) << 16) | (u32::from(pixel.g) << 8) | u32::from(pixel.b);
        }
        buffer.present()?;
        Ok(())
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let window = Rc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("Winkin — text editor")
                        .with_theme(Some(winit::window::Theme::Dark))
                        .with_inner_size(LogicalSize::new(900.0, 660.0)),
                )
                .expect("create editor window"),
        );
        let context = softbuffer::Context::new(window.clone()).expect("create display context");
        self.surface =
            Some(Surface::new(&context, window.clone()).expect("create display surface"));
        window.set_ime_allowed(true);
        window.set_cursor(winit::window::CursorIcon::Text);
        self.window = Some(window);
        let (width, _) = self.viewport();
        self.editor.resize(width - 2.0 * MARGIN);
        self.changed();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self.window.as_ref().is_none_or(|window| window.id() != id) {
            return;
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => {
                if let Err(error) = self.draw() {
                    eprintln!("Render: {error}");
                    event_loop.exit();
                }
            }
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                let (width, _) = self.viewport();
                self.editor.resize(width - 2.0 * MARGIN);
                self.changed();
            }
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                self.key(event.logical_key, event.text.as_deref())
            }
            WindowEvent::Ime(Ime::Preedit(text, cursor)) => {
                self.editor.preedit(&text, cursor);
                self.changed();
            }
            WindowEvent::Ime(Ime::Commit(text)) => {
                self.editor.insert(&text);
                self.changed();
            }
            WindowEvent::Ime(Ime::Disabled) => {
                self.editor.cancel_composition();
                self.changed();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.pointer = position;
                if self.dragging {
                    self.pointer_selection(true);
                }
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                self.dragging = state == ElementState::Pressed;
                if self.dragging {
                    let now = Instant::now();
                    let tolerance = 5.0 * self.window.as_ref().unwrap().scale_factor();
                    let repeated = self.last_click.is_some_and(|(time, point)| {
                        now.duration_since(time) < Duration::from_millis(500)
                            && (point.x - self.pointer.x).hypot(point.y - self.pointer.y)
                                <= tolerance
                    });
                    self.click_count = if repeated {
                        (self.click_count + 1).min(3)
                    } else {
                        1
                    };
                    self.last_click = Some((now, self.pointer));
                    self.pointer_selection(false);
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y * 60.0,
                    MouseScrollDelta::PixelDelta(position) => {
                        position.y as f32 / self.window.as_ref().unwrap().scale_factor() as f32
                    }
                };
                self.last_click = None;
                self.scroll -= dy;
                self.clamp_scroll();
                self.ime_cursor();
                self.window.as_ref().unwrap().request_redraw();
            }
            WindowEvent::Focused(focused) => {
                self.focused = focused;
                self.dragging = false;
                if !focused {
                    self.editor.cancel_composition();
                }
                self.changed();
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.focused {
            if self.blink.elapsed() >= BLINK {
                self.caret = !self.caret;
                self.blink = Instant::now();
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(self.blink + BLINK));
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut file = None;
    let mut screenshot = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--screenshot" => {
                screenshot = Some(PathBuf::from(
                    args.next().ok_or("--screenshot needs a PNG path")?,
                ))
            }
            "--help" | "-h" => {
                println!(
                    "cargo run -p winkin_editor -- [text-file] [--screenshot output.png]\nSave an opened file with Cmd/Ctrl+S. Without a file, opens sample text."
                );
                return Ok(());
            }
            _ if file.is_none() => file = Some(PathBuf::from(arg)),
            _ => return Err(format!("Unexpected argument: {arg}").into()),
        }
    }
    let text = match &file {
        Some(path) => std::fs::read_to_string(path)?,
        None => SAMPLE.into(),
    };
    if let Some(path) = screenshot {
        let mut editor = Editor::new(text);
        editor.resize(836.0);
        let mut renderer = Renderer::new();
        let pixmap = renderer.draw(&editor, 900, 660, 1.0, 0.0, true);
        let mut encoder = png::Encoder::new(std::fs::File::create(&path)?, 900, 660);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()?
            .write_image_data(pixmap.data_as_u8_slice())?;
        println!(
            "{}: {} lines, {} bytes",
            path.display(),
            editor.layout.lines().len(),
            editor.text.len()
        );
        return Ok(());
    }
    EventLoop::new()?.run_app(&mut App::new(text, file))?;
    Ok(())
}
