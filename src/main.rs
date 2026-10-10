use image::imageops::FilterType;
use image::{ImageBuffer, ImageFormat, Rgba};
use serde::{Deserialize, Serialize};
use slint::{
    Color, ComponentHandle, Image as SlintImage, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer,
    SharedString, VecModel,
};
use std::{
    cell::{Cell, RefCell},
    fs,
    path::{Path, PathBuf},
    rc::Rc,
};

slint::include_modules!();

const DEFAULT_SIZE: usize = 32;
const MIN_CUSTOM_SIZE: usize = 32;
const MAX_CUSTOM_SIZE: usize = 256;

#[derive(Serialize, Deserialize)]
struct PixelProject {
    format: String,
    width: usize,
    height: usize,
    pixels: Vec<[u8; 4]>,
}

struct EditorState {
    pixels: Rc<VecModel<Color>>,
    size: Cell<usize>,
    undo: RefCell<Vec<Vec<Color>>>,
    redo: RefCell<Vec<Vec<Color>>>,
    current_path: RefCell<Option<PathBuf>>,
    stroke_start: RefCell<Option<Vec<Color>>>,
    last_painted: Cell<Option<(i32, i32)>>,
}

impl EditorState {
    fn new(pixels: Rc<VecModel<Color>>, size: usize) -> Self {
        Self {
            pixels,
            size: Cell::new(size),
            undo: RefCell::new(Vec::new()),
            redo: RefCell::new(Vec::new()),
            current_path: RefCell::new(None),
            stroke_start: RefCell::new(None),
            last_painted: Cell::new(None),
        }
    }

    fn snapshot(&self) -> Vec<Color> {
        self.pixels.iter().collect()
    }

    fn replace_pixels(&self, colors: &[Color]) {
        self.pixels.set_vec(colors.to_vec());
    }

    fn reset_canvas(&self, size: usize) {
        self.size.set(size);
        self.pixels.set_vec(vec![transparent(); size * size]);
        self.undo.borrow_mut().clear();
        self.redo.borrow_mut().clear();
        self.stroke_start.borrow_mut().take();
        self.last_painted.set(None);
    }

    fn paint_to(&self, target_x: i32, target_y: i32, color: Color) {
        let size = self.size.get() as i32;
        let (mut x, mut y) = self.last_painted.get().unwrap_or((target_x, target_y));
        let dx = (target_x - x).abs();
        let step_x = if x < target_x { 1 } else { -1 };
        let dy = -(target_y - y).abs();
        let step_y = if y < target_y { 1 } else { -1 };
        let mut error = dx + dy;

        loop {
            if (0..size).contains(&x) && (0..size).contains(&y) {
                self.pixels
                    .set_row_data((y as usize * size as usize) + x as usize, color);
            }
            if x == target_x && y == target_y {
                break;
            }
            let doubled_error = error * 2;
            if doubled_error >= dy {
                error += dy;
                x += step_x;
            }
            if doubled_error <= dx {
                error += dx;
                y += step_y;
            }
        }

        self.last_painted.set(Some((target_x, target_y)));
    }

    fn push_undo(&self, snapshot: Vec<Color>) {
        let mut undo = self.undo.borrow_mut();
        undo.push(snapshot);
        if undo.len() > 80 {
            undo.remove(0);
        }
        self.redo.borrow_mut().clear();
    }
}

fn transparent() -> Color {
    Color::from_argb_u8(0, 0, 0, 0)
}

fn color_to_rgba(color: Color) -> [u8; 4] {
    [color.red(), color.green(), color.blue(), color.alpha()]
}

fn rgba_to_color(rgba: [u8; 4]) -> Color {
    Color::from_argb_u8(rgba[3], rgba[0], rgba[1], rgba[2])
}

fn save_project(path: &Path, state: &EditorState) -> Result<(), String> {
    let size = state.size.get();
    let project = PixelProject {
        format: "pixie-1".into(),
        width: size,
        height: size,
        pixels: state.snapshot().into_iter().map(color_to_rgba).collect(),
    };
    let json = serde_json::to_string_pretty(&project).map_err(|error| error.to_string())?;
    fs::write(path, json).map_err(|error| error.to_string())
}

fn load_project(path: &Path) -> Result<(usize, Vec<Color>), String> {
    let json = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let project: PixelProject = serde_json::from_str(&json).map_err(|error| error.to_string())?;
    if project.format != "pixie-1"
        || project.width != project.height
        || !(project.width == 16 || (MIN_CUSTOM_SIZE..=MAX_CUSTOM_SIZE).contains(&project.width))
        || project.pixels.len() != project.width * project.height
    {
        return Err(
            "Pixie projects must be square and between 32 × 32 and 256 × 256 (legacy 16 × 16 is also supported)"
                .into(),
        );
    }
    Ok((
        project.width,
        project.pixels.into_iter().map(rgba_to_color).collect(),
    ))
}

fn export_png(path: &Path, state: &EditorState) -> Result<(), String> {
    const SCALE: u32 = 16;
    let size = state.size.get();
    let colors = state.snapshot();
    let image = ImageBuffer::from_fn((size as u32) * SCALE, (size as u32) * SCALE, |x, y| {
        let source_x = (x / SCALE) as usize;
        let source_y = (y / SCALE) as usize;
        Rgba(color_to_rgba(colors[source_y * size + source_x]))
    });
    image.save(path).map_err(|error| error.to_string())
}

fn file_name(path: &Path) -> SharedString {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Untitled.pixel.json")
        .into()
}

fn set_status(ui: &slint::Weak<App>, message: impl Into<SharedString>) {
    if let Some(ui) = ui.upgrade() {
        ui.set_status_text(message.into());
    }
}

fn canvas_preview(state: &EditorState) -> SlintImage {
    let size = state.size.get();
    let colors = state.snapshot();
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(size as u32, size as u32);

    for (index, output) in buffer.make_mut_slice().iter_mut().enumerate() {
        let x = index % size;
        let y = index / size;
        let checker = if (x + y) % 2 == 0 {
            [232_u8, 235_u8, 240_u8]
        } else {
            [205_u8, 211_u8, 220_u8]
        };
        let foreground = color_to_rgba(colors[index]);
        let alpha = foreground[3] as u16;
        let blend = |channel: usize| {
            ((foreground[channel] as u16 * alpha + checker[channel] as u16 * (255 - alpha)) / 255)
                as u8
        };
        *output = Rgba8Pixel {
            r: blend(0),
            g: blend(1),
            b: blend(2),
            a: 255,
        };
    }

    SlintImage::from_rgba8(buffer)
}

fn refresh_canvas(ui: &App, state: &EditorState) {
    ui.set_canvas_image(canvas_preview(state));
}

fn read_image(path: &PathBuf, state: &EditorState) -> Result<(), Box<dyn std::error::Error>> {
    let size = state.size.get();
    let img = image::open(path)?;
    let img = img.resize_exact(size as u32, size as u32, FilterType::Triangle);
    let img = img.to_rgba8();
    for x in 0..img.width() {
        for y in 0..img.height() {
            let [r, g, b, a] = img.get_pixel(x, y).0;
            state.pixels.set_row_data(
                (x + size as u32 * y) as usize,
                Color::from_argb_u8(a, r, g, b),
            );
        }
    }
    Ok(())
}

pub fn list_all_support_image_format() -> Vec<&'static str> {
    ImageFormat::all()
        .filter(|format| format.reading_enabled())
        .flat_map(|format| format.extensions_str().iter())
        .copied()
        .collect()
}
fn main() -> Result<(), slint::PlatformError> {
    let ui = App::new()?;

    let pixel_model = Rc::new(VecModel::from(vec![
        transparent();
        DEFAULT_SIZE * DEFAULT_SIZE
    ]));
    ui.set_pixels(ModelRc::from(pixel_model.clone()));

    let palette = [
        0x16171a, 0x343a40, 0x868e96, 0xf8f9fa, 0xff595e, 0xff924c, 0xffca3a, 0x8ac926, 0x52b788,
        0x1982c4, 0x4267ac, 0x6a4c93, 0xb5179e, 0xf72585, 0x8d5524, 0xffc3a0,
    ]
    .into_iter()
    .map(|rgb| Color::from_rgb_u8((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8))
    .collect::<Vec<_>>();
    ui.set_palette(ModelRc::new(VecModel::from(palette)));

    let state = Rc::new(EditorState::new(pixel_model, DEFAULT_SIZE));
    refresh_canvas(&ui, &state);

    {
        let weak = ui.as_weak();
        ui.on_choose_color(move |color| {
            if let Some(ui) = weak.upgrade() {
                ui.set_active_color(color);
                ui.set_color_red(color.red() as f32);
                ui.set_color_green(color.green() as f32);
                ui.set_color_blue(color.blue() as f32);
            }
        });
    }

    {
        let state = state.clone();
        let weak = ui.as_weak();
        ui.on_begin_stroke(move || {
            if let Some(ui) = weak.upgrade() {
                if ui.get_tool() != 2 {
                    *state.stroke_start.borrow_mut() = Some(state.snapshot());
                    state.last_painted.set(None);
                }
            }
        });
    }

    {
        let state = state.clone();
        let weak = ui.as_weak();
        ui.on_end_stroke(move || {
            state.last_painted.set(None);
            if let Some(before) = state.stroke_start.borrow_mut().take() {
                if before != state.snapshot() {
                    state.push_undo(before);
                    if let Some(ui) = weak.upgrade() {
                        ui.set_modified(true);
                        ui.set_status_text("Pixel updated".into());
                    }
                }
            }
        });
    }

    {
        let state = state.clone();
        let weak = ui.as_weak();
        ui.on_paint(move |x, y| {
            let size = state.size.get();
            if !(0..size as i32).contains(&x) || !(0..size as i32).contains(&y) {
                return;
            }
            if let Some(ui) = weak.upgrade() {
                let index = y as usize * size + x as usize;
                match ui.get_tool() {
                    0 => {
                        state.paint_to(x, y, ui.get_active_color());
                        refresh_canvas(&ui, &state);
                    }
                    1 => {
                        state.paint_to(x, y, transparent());
                        refresh_canvas(&ui, &state);
                    }
                    2 => {
                        if let Some(color) = state.pixels.row_data(index) {
                            ui.invoke_choose_color(color);
                            ui.set_tool(0);
                            ui.set_status_text("Color picked".into());
                        }
                    }
                    _ => {}
                }
            }
        });
    }

    {
        let state = state.clone();
        let weak = ui.as_weak();
        ui.on_pick_color(move |x, y| {
            let size = state.size.get();
            if !(0..size as i32).contains(&x) || !(0..size as i32).contains(&y) {
                return;
            }
            if let (Some(ui), Some(color)) = (
                weak.upgrade(),
                state.pixels.row_data(y as usize * size + x as usize),
            ) {
                ui.invoke_choose_color(color);
                ui.set_status_text("Color picked".into());
            }
        });
    }

    {
        let state = state.clone();
        let weak = ui.as_weak();
        ui.on_undo(move || {
            if let Some(previous) = state.undo.borrow_mut().pop() {
                state.redo.borrow_mut().push(state.snapshot());
                state.replace_pixels(&previous);
                if let Some(ui) = weak.upgrade() {
                    refresh_canvas(&ui, &state);
                    ui.set_modified(true);
                    ui.set_status_text("Undo".into());
                }
            } else {
                set_status(&weak, "Nothing to undo");
            }
        });
    }

    {
        let state = state.clone();
        let weak = ui.as_weak();
        ui.on_redo(move || {
            if let Some(next) = state.redo.borrow_mut().pop() {
                state.undo.borrow_mut().push(state.snapshot());
                state.replace_pixels(&next);
                if let Some(ui) = weak.upgrade() {
                    refresh_canvas(&ui, &state);
                    ui.set_modified(true);
                    ui.set_status_text("Redo".into());
                }
            } else {
                set_status(&weak, "Nothing to redo");
            }
        });
    }

    {
        let state = state.clone();
        let weak = ui.as_weak();
        ui.on_clear_canvas(move || {
            let before = state.snapshot();
            if before.iter().any(|color| color.alpha() != 0) {
                state.push_undo(before);
                let size = state.size.get();
                state.replace_pixels(&vec![transparent(); size * size]);
                if let Some(ui) = weak.upgrade() {
                    refresh_canvas(&ui, &state);
                    ui.set_modified(true);
                    ui.set_status_text("Canvas cleared".into());
                }
            }
        });
    }

    {
        let state = state.clone();
        let weak = ui.as_weak();
        ui.on_new_project(move || {
            let size = state.size.get().max(MIN_CUSTOM_SIZE);
            state.reset_canvas(size);
            *state.current_path.borrow_mut() = None;
            if let Some(ui) = weak.upgrade() {
                refresh_canvas(&ui, &state);
                ui.set_canvas_size(size as i32);
                ui.set_custom_size_text(size.to_string().into());
                ui.set_document_name("Untitled.pixel.json".into());
                ui.set_modified(false);
                ui.set_status_text(format!("New {size} × {size} project").into());
            }
        });
    }

    {
        let state = state.clone();
        let weak = ui.as_weak();
        ui.on_resize_canvas(move |requested_size| {
            let size = requested_size as usize;
            if !(MIN_CUSTOM_SIZE..=MAX_CUSTOM_SIZE).contains(&size) {
                set_status(&weak, "Canvas size must be between 32 and 256");
                return;
            }
            if size == state.size.get() {
                return;
            }
            state.reset_canvas(size);
            *state.current_path.borrow_mut() = None;
            if let Some(ui) = weak.upgrade() {
                refresh_canvas(&ui, &state);
                ui.set_canvas_size(size as i32);
                ui.set_custom_size_text(size.to_string().into());
                ui.set_document_name("Untitled.pixel.json".into());
                ui.set_modified(false);
                ui.set_status_text(format!("New {size} × {size} canvas").into());
            }
        });
    }

    {
        let state = state.clone();
        let weak = ui.as_weak();
        ui.on_resize_custom(move |text| {
            let Ok(size) = text.as_str().trim().parse::<usize>() else {
                set_status(&weak, "Enter a whole number from 32 to 256");
                return;
            };
            if !(MIN_CUSTOM_SIZE..=MAX_CUSTOM_SIZE).contains(&size) {
                set_status(&weak, "Canvas size must be between 32 and 256");
                return;
            }
            if size == state.size.get() {
                set_status(&weak, format!("Canvas is already {size} × {size}"));
                return;
            }

            state.reset_canvas(size);
            *state.current_path.borrow_mut() = None;
            if let Some(ui) = weak.upgrade() {
                refresh_canvas(&ui, &state);
                ui.set_canvas_size(size as i32);
                ui.set_custom_size_text(size.to_string().into());
                ui.set_document_name("Untitled.pixel.json".into());
                ui.set_modified(false);
                ui.set_status_text(format!("New custom {size} × {size} canvas").into());
            }
        });
    }

    {
        let state = state.clone();
        let weak = ui.as_weak();
        ui.on_open_project(move || {
            let Some(path) = rfd::FileDialog::new()
                .add_filter("Pixie project", &["json"])
                .pick_file()
            else {
                return;
            };
            match load_project(&path) {
                Ok((size, colors)) => {
                    state.size.set(size);
                    state.replace_pixels(&colors);
                    state.undo.borrow_mut().clear();
                    state.redo.borrow_mut().clear();
                    *state.current_path.borrow_mut() = Some(path.clone());
                    if let Some(ui) = weak.upgrade() {
                        refresh_canvas(&ui, &state);
                        ui.set_canvas_size(size as i32);
                        ui.set_custom_size_text(size.to_string().into());
                        ui.set_document_name(file_name(&path));
                        ui.set_modified(false);
                        ui.set_status_text(format!("Opened {size} × {size} project").into());
                    }
                }
                Err(error) => set_status(&weak, format!("Open failed: {error}")),
            }
        });
    }

    {
        let state = state.clone();
        let weak = ui.as_weak();
        ui.on_save_project(move || {
            let path = state.current_path.borrow().clone().or_else(|| {
                rfd::FileDialog::new()
                    .add_filter("Pixie project", &["json"])
                    .set_file_name("artwork.pixel.json")
                    .save_file()
            });
            let Some(path) = path else {
                return;
            };
            match save_project(&path, &state) {
                Ok(()) => {
                    *state.current_path.borrow_mut() = Some(path.clone());
                    if let Some(ui) = weak.upgrade() {
                        ui.set_document_name(file_name(&path));
                        ui.set_modified(false);
                        ui.set_status_text("Project saved".into());
                    }
                }
                Err(error) => set_status(&weak, format!("Save failed: {error}")),
            }
        });
    }

    {
        let state = state.clone();
        let weak = ui.as_weak();
        ui.on_save_project_as(move || {
            let Some(path) = rfd::FileDialog::new()
                .add_filter("Pixie project", &["json"])
                .set_file_name("artwork.pixel.json")
                .save_file()
            else {
                return;
            };
            match save_project(&path, &state) {
                Ok(()) => {
                    *state.current_path.borrow_mut() = Some(path.clone());
                    if let Some(ui) = weak.upgrade() {
                        ui.set_document_name(file_name(&path));
                        ui.set_modified(false);
                        ui.set_status_text("Project saved".into());
                    }
                }
                Err(error) => set_status(&weak, format!("Save failed: {error}")),
            }
        });
    }

    {
        let state = state.clone();
        let weak = ui.as_weak();
        ui.on_export_png(move || {
            let Some(path) = rfd::FileDialog::new()
                .add_filter("PNG image", &["png"])
                .set_file_name("pixel-art.png")
                .save_file()
            else {
                return;
            };
            match export_png(&path, &state) {
                Ok(()) => {
                    let output_size = state.size.get() * 16;
                    set_status(
                        &weak,
                        format!("PNG exported at {output_size} × {output_size}"),
                    );
                }
                Err(error) => set_status(&weak, format!("Export failed: {error}")),
            }
        });
    }
    {
        let state = state.clone();
        let weak = ui.as_weak();
        ui.on_read(move || {
            let Some(path) = rfd::FileDialog::new()
                .add_filter("image", list_all_support_image_format().as_slice())
                .pick_file()
            else {
                return;
            };
            let before = state.snapshot();
            match read_image(&path, &state) {
                Ok(_) => {
                    state.push_undo(before);
                    if let Some(ui) = weak.upgrade() {
                        refresh_canvas(&ui, &state);
                        ui.set_modified(true);
                        let size = state.size.get();
                        ui.set_status_text(format!("Image fitted to {size} × {size}").into());
                    }
                }
                Err(err) => {
                    set_status(&weak, format!("Read failed: {err}"));
                }
            };
        });
    }

    ui.run()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_and_png_use_custom_canvas_size() {
        let model = Rc::new(VecModel::from(vec![transparent(); 48 * 48]));
        model.set_row_data(47 * 48 + 47, Color::from_rgb_u8(255, 0, 0));
        let state = EditorState::new(model, 48);

        let unique = format!("pixie-size-test-{}", std::process::id());
        let project_path = std::env::temp_dir().join(format!("{unique}.json"));
        let png_path = std::env::temp_dir().join(format!("{unique}.png"));

        save_project(&project_path, &state).unwrap();
        let (size, pixels) = load_project(&project_path).unwrap();
        assert_eq!(size, 48);
        assert_eq!(pixels.len(), 48 * 48);

        export_png(&png_path, &state).unwrap();
        let image = image::open(&png_path).unwrap();
        assert_eq!((image.width(), image.height()), (768, 768));

        let _ = fs::remove_file(project_path);
        let _ = fs::remove_file(png_path);
    }

    #[test]
    fn continuous_stroke_fills_pixels_between_pointer_events() {
        let model = Rc::new(VecModel::from(vec![transparent(); 64 * 64]));
        let state = EditorState::new(model, 64);
        let ink = Color::from_rgb_u8(20, 40, 60);

        state.paint_to(2, 7, ink);
        state.paint_to(14, 7, ink);

        for x in 2..=14 {
            assert_eq!(state.pixels.row_data(7 * 64 + x), Some(ink));
        }
    }
}
