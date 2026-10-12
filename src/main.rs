use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::Duration;

use eframe::egui;
use pdf2img::{
    compress_to, convert_pdf_with_progress, height_for_width, output_jpeg_path, read_dimensions,
    unsupported_reason, width_for_height, CompressMode, ConvertOptions, ConvertProgress,
    OutputFormat, SizeLimits,
};

fn main() -> eframe::Result {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([780.0, 900.0])
            .with_min_inner_size([680.0, 760.0])
            .with_title("PDF 转图片"),
        ..Default::default()
    };
    eframe::run_native(
        "PDF 转图片",
        native_options,
        Box::new(|cc| Ok(Box::new(PdfApp::new(cc)))),
    )
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct Persisted {
    pdf_path: String,
    format: String,
    min_width: String,
    max_width: String,
    min_height: String,
    max_height: String,
    dpi: String,
    quality: u8,
    pages: String,
    #[serde(default)]
    page: String,
    #[serde(default)]
    compress_choice: String,
    #[serde(default)]
    size_limit: String,
    #[serde(default)]
    size_unit: String,
    #[serde(default = "default_true")]
    aspect_locked: bool,
}

fn default_true() -> bool {
    true
}

struct PdfApp {
    pdf_path: String,
    format: OutputFormat,
    min_width: String,
    max_width: String,
    min_height: String,
    max_height: String,
    dpi: String,
    quality: u8,
    pages: String,
    password: String,
    page: AppPage,
    compress_choice: CompressChoice,
    size_limit: String,
    size_unit: SizeUnit,
    aspect_locked: bool,
    images: Vec<SourceImage>,
    selected_image: usize,
    lines: Vec<LogLine>,
    converting: bool,
    rx: Option<Receiver<WorkerMsg>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AppPage {
    Pdf,
    Compress,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CompressChoice {
    Normal,
    HighQuality,
    MaxSize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SizeUnit {
    Kb,
    Mb,
}

struct SourceImage {
    path: PathBuf,
    source_w: u32,
    source_h: u32,
    out_w: String,
    out_h: String,
}

struct LogLine {
    kind: LogKind,
    text: String,
}

enum LogKind {
    Info,
    Warn,
    Error,
}

enum WorkerMsg {
    Progress(ConvertProgress),
    Compressed(CompressedImage),
    Finished(String),
    Failed(String),
}

struct CompressedImage {
    index: usize,
    total: usize,
    path: PathBuf,
    width: u32,
    height: u32,
    bytes: u64,
}

impl PdfApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_cjk_font(&cc.egui_ctx);
        let persisted = cc
            .storage
            .and_then(|storage| eframe::get_value::<Persisted>(storage, "settings"));
        let mut app = Self::from_persisted(persisted);
        if let Some(path) = std::env::args().nth(1) {
            if Path::new(&path).is_file() {
                app.pdf_path = path;
            }
        }
        app
    }

    fn from_persisted(persisted: Option<Persisted>) -> Self {
        let persisted = persisted.unwrap_or(Persisted {
            pdf_path: String::new(),
            format: "png".to_string(),
            min_width: String::new(),
            max_width: String::new(),
            min_height: String::new(),
            max_height: String::new(),
            dpi: "144".to_string(),
            quality: 85,
            pages: String::new(),
            page: String::new(),
            compress_choice: String::new(),
            size_limit: String::new(),
            size_unit: String::new(),
            aspect_locked: true,
        });
        Self {
            pdf_path: persisted.pdf_path,
            format: parse_format(&persisted.format),
            min_width: persisted.min_width,
            max_width: persisted.max_width,
            min_height: persisted.min_height,
            max_height: persisted.max_height,
            dpi: persisted.dpi,
            quality: persisted.quality.clamp(1, 100),
            pages: persisted.pages,
            password: String::new(),
            page: if persisted.page == "compress" {
                AppPage::Compress
            } else {
                AppPage::Pdf
            },
            compress_choice: parse_compress_choice(&persisted.compress_choice),
            size_limit: if persisted.size_limit.is_empty() {
                "1.00".to_string()
            } else {
                persisted.size_limit
            },
            size_unit: if persisted.size_unit.eq_ignore_ascii_case("kb") {
                SizeUnit::Kb
            } else {
                SizeUnit::Mb
            },
            aspect_locked: persisted.aspect_locked,
            images: Vec::new(),
            selected_image: 0,
            lines: Vec::new(),
            converting: false,
            rx: None,
        }
    }

    fn persisted(&self) -> Persisted {
        Persisted {
            pdf_path: self.pdf_path.clone(),
            format: format_key(self.format).to_string(),
            min_width: self.min_width.clone(),
            max_width: self.max_width.clone(),
            min_height: self.min_height.clone(),
            max_height: self.max_height.clone(),
            dpi: self.dpi.clone(),
            quality: self.quality,
            pages: self.pages.clone(),
            page: match self.page {
                AppPage::Pdf => "pdf",
                AppPage::Compress => "compress",
            }
            .to_string(),
            compress_choice: match self.compress_choice {
                CompressChoice::Normal => "normal",
                CompressChoice::HighQuality => "high",
                CompressChoice::MaxSize => "max",
            }
            .to_string(),
            size_limit: self.size_limit.clone(),
            size_unit: match self.size_unit {
                SizeUnit::Kb => "kb",
                SizeUnit::Mb => "mb",
            }
            .to_string(),
            aspect_locked: self.aspect_locked,
        }
    }

    fn poll(&mut self) {
        let Some(rx) = self.rx.take() else {
            return;
        };
        let mut restore = true;
        loop {
            match rx.try_recv() {
                Ok(WorkerMsg::Progress(progress)) => {
                    self.push(
                        LogKind::Info,
                        format!(
                            "已保存 {}（{}×{}）  {}/{}",
                            progress.image.path.display(),
                            progress.image.width,
                            progress.image.height,
                            progress.index,
                            progress.total
                        ),
                    );
                    if progress.image.below_minimum {
                        self.push(
                            LogKind::Warn,
                            format!(
                                "第 {} 页在不超过上限时无法同时达到最小宽高，实际为 {}×{}",
                                progress.image.page, progress.image.width, progress.image.height
                            ),
                        );
                    }
                }
                Ok(WorkerMsg::Compressed(image)) => {
                    if let Some(job) = self.images.get_mut(image.index) {
                        job.out_w = image.width.to_string();
                        job.out_h = image.height.to_string();
                    }
                    self.push(
                        LogKind::Info,
                        format!(
                            "已保存 {}（{}×{}，{}）  {}/{}",
                            image.path.display(),
                            image.width,
                            image.height,
                            format_bytes(image.bytes),
                            image.index + 1,
                            image.total
                        ),
                    );
                }
                Ok(WorkerMsg::Finished(message)) => {
                    self.push(LogKind::Info, message);
                    self.converting = false;
                    restore = false;
                    break;
                }
                Ok(WorkerMsg::Failed(error)) => {
                    self.push(LogKind::Error, error);
                    self.converting = false;
                    restore = false;
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if self.converting {
                        self.push(LogKind::Error, "转换意外中断".to_string());
                        self.converting = false;
                    }
                    restore = false;
                    break;
                }
            }
        }
        if restore {
            self.rx = Some(rx);
        }
    }

    fn accept_dropped_files(&mut self, ctx: &egui::Context) {
        if self.converting {
            return;
        }
        let dropped: Vec<PathBuf> = ctx.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_path_buf())
                .collect()
        });
        if dropped.is_empty() {
            return;
        }
        match self.page {
            AppPage::Pdf => {
                if let Some(path) = dropped.into_iter().find(|path| is_pdf(path)) {
                    self.pdf_path = path.display().to_string();
                } else {
                    self.push(LogKind::Error, "请拖入 PDF 文件".to_string());
                }
            }
            AppPage::Compress => self.add_image_paths(dropped),
        }
    }

    fn start_conversion(&mut self, ctx: egui::Context) {
        let options = match self.build_options() {
            Ok(options) => options,
            Err(error) => {
                self.lines.clear();
                self.push(LogKind::Error, error);
                return;
            }
        };
        let path = PathBuf::from(self.pdf_path.trim());
        self.lines.clear();
        self.push(LogKind::Info, "开始转换…".to_string());
        self.converting = true;

        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        let ctx_done = ctx.clone();
        std::thread::spawn(move || {
            let ctx_page = ctx.clone();
            let result = convert_pdf_with_progress(&path, &options, |progress| {
                let _ = tx.send(WorkerMsg::Progress(progress));
                ctx_page.request_repaint();
            });
            let _ = match result {
                Ok(images) => tx.send(WorkerMsg::Finished(format!("完成，共 {} 页", images.len()))),
                Err(error) => tx.send(WorkerMsg::Failed(format!("转换失败：{error:#}"))),
            };
            ctx_done.request_repaint();
        });
    }

    fn build_options(&self) -> Result<ConvertOptions, String> {
        let pdf = self.pdf_path.trim();
        if pdf.is_empty() {
            return Err("请先选择 PDF 文件".to_string());
        }
        if !Path::new(pdf).is_file() {
            return Err(format!("找不到 PDF 文件：{pdf}"));
        }

        Ok(ConvertOptions {
            format: self.format,
            limits: SizeLimits {
                min_width: optional_u32(&self.min_width, "最小宽度")?,
                max_width: optional_u32(&self.max_width, "最大宽度")?,
                min_height: optional_u32(&self.min_height, "最小高度")?,
                max_height: optional_u32(&self.max_height, "最大高度")?,
            },
            dpi: parse_dpi(&self.dpi)?,
            quality: self.quality.clamp(1, 100),
            pages: nonempty(&self.pages),
            password: nonempty(&self.password),
        })
    }

    fn pick_pdf(&mut self) {
        let mut dialog = rfd::FileDialog::new().add_filter("PDF 文件", &["pdf"]);
        if let Some(dir) = parent_dir(&self.pdf_path) {
            if dir.is_dir() {
                dialog = dialog.set_directory(dir);
            }
        }
        if let Some(path) = dialog.pick_file() {
            self.pdf_path = path.display().to_string();
        }
    }

    fn open_output_dir(&self) {
        let dir = match self.page {
            AppPage::Pdf => parent_dir(&self.pdf_path),
            AppPage::Compress => self
                .images
                .get(self.selected_image)
                .and_then(|image| image.path.parent())
                .filter(|dir| !dir.as_os_str().is_empty())
                .map(Path::to_path_buf)
                .or_else(|| {
                    self.images
                        .first()
                        .and_then(|image| image.path.parent())
                        .map(Path::to_path_buf)
                }),
        };
        let Some(dir) = dir else {
            return;
        };
        let _ = std::process::Command::new("explorer").arg(dir).spawn();
    }

    fn add_image_paths(&mut self, paths: Vec<PathBuf>) {
        let mut added = 0;
        for path in paths {
            if self.images.iter().any(|image| image.path == path) {
                continue;
            }
            if let Some(reason) = unsupported_reason(&path) {
                self.push(LogKind::Error, format!("{}：{reason}", path.display()));
                continue;
            }
            match read_dimensions(&path) {
                Ok((width, height)) => {
                    self.images.push(SourceImage {
                        path,
                        source_w: width,
                        source_h: height,
                        out_w: width.to_string(),
                        out_h: height.to_string(),
                    });
                    added += 1;
                }
                Err(error) => self.push(LogKind::Error, format!("{error:#}")),
            }
        }
        if added > 0 {
            self.selected_image = self.images.len() - 1;
        }
    }

    fn pick_images(&mut self) {
        let mut dialog = rfd::FileDialog::new().add_filter("图片", &["png", "jpg", "jpeg", "webp", "bmp"]);
        if let Some(image) = self.images.get(self.selected_image) {
            if let Some(dir) = image.path.parent() {
                if dir.is_dir() {
                    dialog = dialog.set_directory(dir);
                }
            }
        }
        if let Some(paths) = dialog.pick_files() {
            self.add_image_paths(paths);
        }
    }

    fn start_compress(&mut self, ctx: egui::Context) {
        if self.images.is_empty() {
            self.lines.clear();
            self.push(LogKind::Error, "请先选择要压缩的图片".to_string());
            return;
        }
        self.apply_aspect_to_selected();
        let mode = match self.compress_mode() {
            Ok(mode) => mode,
            Err(error) => {
                self.lines.clear();
                self.push(LogKind::Error, error);
                return;
            }
        };
        let mut jobs = Vec::new();
        for image in &self.images {
            let width = match image.out_w.trim().parse::<u32>() {
                Ok(width) if width > 0 => width,
                _ => {
                    self.lines.clear();
                    self.push(
                        LogKind::Error,
                        format!("{} 的宽度必须是正整数", image.path.display()),
                    );
                    return;
                }
            };
            let height = match image.out_h.trim().parse::<u32>() {
                Ok(height) if height > 0 => height,
                _ => {
                    self.lines.clear();
                    self.push(
                        LogKind::Error,
                        format!("{} 的高度必须是正整数", image.path.display()),
                    );
                    return;
                }
            };
            jobs.push((image.path.clone(), width, height, output_jpeg_path(&image.path)));
        }

        self.lines.clear();
        self.push(LogKind::Info, "开始压缩…".to_string());
        self.converting = true;
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        let ctx_done = ctx.clone();
        let total = jobs.len();
        std::thread::spawn(move || {
            let ctx_page = ctx.clone();
            for (index, (src, width, height, dest)) in jobs.into_iter().enumerate() {
                match compress_to(&src, &dest, width, height, mode) {
                    Ok(result) => {
                        let _ = tx.send(WorkerMsg::Compressed(CompressedImage {
                            index,
                            total,
                            path: dest,
                            width: result.width,
                            height: result.height,
                            bytes: result.bytes,
                        }));
                        ctx_page.request_repaint();
                    }
                    Err(error) => {
                        let _ = tx.send(WorkerMsg::Failed(format!(
                            "压缩失败：{}：{error:#}",
                            src.display()
                        )));
                        ctx_done.request_repaint();
                        return;
                    }
                }
            }
            let _ = tx.send(WorkerMsg::Finished(format!("完成，共 {total} 张")));
            ctx_done.request_repaint();
        });
    }

    fn compress_mode(&self) -> Result<CompressMode, String> {
        match self.compress_choice {
            CompressChoice::Normal => Ok(CompressMode::Normal),
            CompressChoice::HighQuality => Ok(CompressMode::HighQuality),
            CompressChoice::MaxSize => Ok(CompressMode::MaxBytes(parse_size_limit(
                &self.size_limit,
                self.size_unit,
            )?)),
        }
    }

    fn apply_aspect_to_selected(&mut self) {
        if !self.aspect_locked {
            return;
        }
        let Some(image) = self.images.get_mut(self.selected_image) else {
            return;
        };
        if let Ok(width) = image.out_w.trim().parse::<u32>() {
            image.out_h = height_for_width(image.source_w, image.source_h, width).to_string();
        }
    }

    fn push(&mut self, kind: LogKind, text: String) {
        self.lines.push(LogLine { kind, text });
    }
}

impl eframe::App for PdfApp {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, "settings", &self.persisted());
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.accept_dropped_files(ctx);
        self.poll();
        if self.converting {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.page, AppPage::Pdf, "PDF 转图片");
                ui.selectable_value(&mut self.page, AppPage::Compress, "图片压缩");
            });
            ui.add_space(8.0);
            match self.page {
                AppPage::Pdf => self.pdf_ui(ui),
                AppPage::Compress => self.compress_ui(ui),
            }
            self.log_ui(ui);
        });
    }
}

impl PdfApp {
    fn pdf_ui(&mut self, ui: &mut egui::Ui) {
            ui.heading("PDF 转图片");
            ui.label("选择 PDF，设置格式和像素范围。图片按页面比例生成，并保存在 PDF 所在文件夹。");
            ui.add_space(12.0);

            ui.horizontal(|ui| {
                ui.label("PDF 文件");
                ui.add_enabled(
                    !self.converting,
                    egui::TextEdit::singleline(&mut self.pdf_path)
                        .desired_width((ui.available_width() - 88.0).max(160.0))
                        .hint_text("选择文件，或把 PDF 拖进窗口"),
                );
                if ui
                    .add_enabled(!self.converting, egui::Button::new("浏览…"))
                    .clicked()
                {
                    self.pick_pdf();
                }
            });
            if let Some(dir) = parent_dir(&self.pdf_path) {
                ui.weak(format!("图片将保存到 {}", dir.display()));
            } else {
                ui.weak("图片将保存到 PDF 所在文件夹");
            }
            if self.page == AppPage::Pdf && !ui.ctx().input(|input| input.raw.hovered_files.is_empty()) {
                ui.colored_label(
                    egui::Color32::from_rgb(37, 99, 235),
                    "松开鼠标，使用拖入的 PDF",
                );
            }

            ui.add_space(8.0);
            ui.add_enabled_ui(!self.converting, |ui| {
                ui.horizontal(|ui| {
                    ui.label("图片格式");
                    egui::ComboBox::from_id_salt("format")
                        .selected_text(self.format.label())
                        .show_ui(ui, |ui| {
                            for format in [
                                OutputFormat::Png,
                                OutputFormat::Jpeg,
                                OutputFormat::Webp,
                                OutputFormat::Bmp,
                            ] {
                                ui.selectable_value(&mut self.format, format, format.label());
                            }
                        });
                });

                ui.horizontal(|ui| {
                    ui.label("页码");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.pages)
                            .desired_width(220.0)
                            .hint_text("全部，例如 1-3,5"),
                    );
                });
                ui.horizontal(|ui| {
                    ui.label("密码");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.password)
                            .password(true)
                            .desired_width(220.0)
                            .hint_text("未加密则留空"),
                    );
                });

                ui.add_space(10.0);
                ui.strong("图片大小（像素）");
                ui.weak("留空表示不限制。页面保持原始宽高比，不会被拉变形。");
                ui.add_space(4.0);
                egui::Grid::new("size-limits")
                    .num_columns(4)
                    .spacing([12.0, 8.0])
                    .show(ui, |ui| {
                        number_field(ui, "最小宽度", &mut self.min_width);
                        number_field(ui, "最大宽度", &mut self.max_width);
                        ui.end_row();
                        number_field(ui, "最小高度", &mut self.min_height);
                        number_field(ui, "最大高度", &mut self.max_height);
                        ui.end_row();
                    });

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label("分辨率");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.dpi)
                            .desired_width(80.0)
                            .hint_text("144"),
                    );
                    ui.weak("未达到上限时使用，默认 144");
                });

                ui.add_space(4.0);
                let jpeg = self.format == OutputFormat::Jpeg;
                ui.add_enabled_ui(jpeg, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("JPEG 质量");
                        ui.add(egui::Slider::new(&mut self.quality, 1..=100));
                    });
                });
                if !jpeg {
                    ui.weak("质量滑块只对 JPEG 生效。WebP 使用无损编码。");
                }
            });

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let convert = ui.add_enabled(!self.converting, egui::Button::new("开始转换"));
                if convert.clicked() {
                    self.start_conversion(ui.ctx().clone());
                }
                if self.converting {
                    ui.spinner();
                    ui.label("正在转换，请稍候…");
                }
                if ui
                    .add_enabled(
                        parent_dir(&self.pdf_path).is_some(),
                        egui::Button::new("打开所在文件夹"),
                    )
                    .clicked()
                {
                    self.open_output_dir();
                }
            });

    }

    fn compress_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("图片压缩");
        ui.horizontal(|ui| {
            ui.label("压缩方式");
            ui.weak("不支持 GIF、SVG 格式");
        });
        ui.add_space(4.0);

        let busy = self.converting;
        ui.add_enabled_ui(!busy, |ui| {
            ui.horizontal(|ui| {
                if ui.button("添加图片").clicked() {
                    self.pick_images();
                }
                if ui
                    .add_enabled(!self.images.is_empty(), egui::Button::new("清空"))
                    .clicked()
                {
                    self.images.clear();
                    self.selected_image = 0;
                }
                ui.weak("也可以把图片拖进窗口");
            });
            if self.images.len() > 1 {
                ui.horizontal(|ui| {
                    ui.label("当前图片");
                    let selected = self
                        .images
                        .get(self.selected_image)
                        .map(|image| image.path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default())
                        .unwrap_or_default();
                    egui::ComboBox::from_id_salt("current-image")
                        .selected_text(selected)
                        .show_ui(ui, |ui| {
                            for (index, image) in self.images.iter().enumerate() {
                                let name = image
                                    .path
                                    .file_name()
                                    .map(|name| name.to_string_lossy().into_owned())
                                    .unwrap_or_else(|| image.path.display().to_string());
                                ui.selectable_value(&mut self.selected_image, index, name);
                            }
                        });
                });
            } else if let Some(image) = self.images.first() {
                ui.label(image.path.display().to_string());
            }

            ui.add_space(8.0);
            ui.radio_value(&mut self.compress_choice, CompressChoice::Normal, "普通压缩");
            ui.horizontal(|ui| {
                ui.add_space(22.0);
                ui.weak("压缩比例小，体积大");
            });
            ui.radio_value(
                &mut self.compress_choice,
                CompressChoice::HighQuality,
                "高品质压缩",
            );
            ui.horizontal(|ui| {
                ui.add_space(22.0);
                ui.weak("在保证清晰度的前提下，压缩到最小");
            });
            ui.radio_value(
                &mut self.compress_choice,
                CompressChoice::MaxSize,
                "每张图片体积小于",
            );
            ui.horizontal(|ui| {
                ui.add_space(22.0);
                ui.add_enabled_ui(self.compress_choice == CompressChoice::MaxSize, |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.size_limit)
                            .desired_width(90.0)
                            .hint_text("1.00"),
                    );
                    egui::ComboBox::from_id_salt("size-unit")
                        .selected_text(match self.size_unit {
                            SizeUnit::Mb => "MB",
                            SizeUnit::Kb => "KB",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.size_unit, SizeUnit::Mb, "MB");
                            ui.selectable_value(&mut self.size_unit, SizeUnit::Kb, "KB");
                        });
                });
            });

            ui.add_space(12.0);
            ui.label("当前图片压缩后分辨率（单位：像素）");
            if self.images.is_empty() {
                ui.weak("添加图片后可以调整压缩后的宽高");
            } else if let Some(image) = self.images.get(self.selected_image) {
                ui.weak(format!(
                    "原始 {}×{}",
                    image.source_w, image.source_h
                ));
            }
            if !self.images.is_empty() {
                let index = self.selected_image.min(self.images.len() - 1);
                self.selected_image = index;
                let mut out_w = self.images[index].out_w.clone();
                let mut out_h = self.images[index].out_h.clone();
                let source_w = self.images[index].source_w;
                let source_h = self.images[index].source_h;
                let mut width_edited = false;
                let mut height_edited = false;
                ui.horizontal(|ui| {
                    let width = ui.add(
                        egui::TextEdit::singleline(&mut out_w)
                            .desired_width(90.0)
                            .hint_text("宽"),
                    );
                    ui.label("宽");
                    if ui
                        .selectable_label(self.aspect_locked, "保持比例")
                        .clicked()
                    {
                        self.aspect_locked = !self.aspect_locked;
                    }
                    let height = ui.add(
                        egui::TextEdit::singleline(&mut out_h)
                            .desired_width(90.0)
                            .hint_text("高"),
                    );
                    ui.label("高");
                    width_edited = width.lost_focus() && width.changed() || width.lost_focus() && out_w != self.images[index].out_w;
                    height_edited = height.lost_focus() && out_h != self.images[index].out_h;
                    let _ = width;
                    let _ = height;
                });
                if self.aspect_locked && width_edited {
                    if let Ok(width) = out_w.trim().parse::<u32>() {
                        out_h = height_for_width(source_w, source_h, width).to_string();
                    }
                } else if self.aspect_locked && height_edited {
                    if let Ok(height) = out_h.trim().parse::<u32>() {
                        out_w = width_for_height(source_w, source_h, height).to_string();
                    }
                }
                if let Some(image) = self.images.get_mut(index) {
                    image.out_w = out_w;
                    image.out_h = out_h;
                }
            }
            if self.page == AppPage::Compress
                && !ui.ctx().input(|input| input.raw.hovered_files.is_empty())
            {
                ui.colored_label(
                    egui::Color32::from_rgb(37, 99, 235),
                    "松开鼠标，添加图片",
                );
            }
        });

        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!self.converting, egui::Button::new("开始压缩"))
                .clicked()
            {
                self.start_compress(ui.ctx().clone());
            }
            if self.converting {
                ui.spinner();
                ui.label("正在压缩，请稍候…");
            }
            if ui
                .add_enabled(!self.images.is_empty(), egui::Button::new("打开所在文件夹"))
                .clicked()
            {
                self.open_output_dir();
            }
        });
        ui.weak("结果保存为 JPEG，放在原图旁边，文件名以 -compressed 结尾。");
    }

    fn log_ui(&mut self, ui: &mut egui::Ui) {
            ui.add_space(8.0);
            ui.separator();
            ui.label(match self.page {
                AppPage::Pdf => "转换记录",
                AppPage::Compress => "压缩记录",
            });
            let log_height = ui.available_height().max(100.0);
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .max_height(log_height)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    if self.lines.is_empty() {
                        ui.weak("还没有记录");
                    }
                    for line in &self.lines {
                        match line.kind {
                            LogKind::Info => ui.label(&line.text),
                            LogKind::Warn => ui.colored_label(
                                egui::Color32::from_rgb(176, 96, 0),
                                &line.text,
                            ),
                            LogKind::Error => ui.colored_label(
                                egui::Color32::from_rgb(176, 32, 32),
                                &line.text,
                            ),
                        };
                    }
                });
    }
}

fn number_field(ui: &mut egui::Ui, label: &str, text: &mut String) {
    ui.label(label);
    ui.add(
        egui::TextEdit::singleline(text)
            .desired_width(110.0)
            .hint_text("不限"),
    );
}

fn optional_u32(text: &str, label: &str) -> Result<Option<u32>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let value: u32 = text
        .parse()
        .map_err(|_| format!("{label}必须是正整数"))?;
    if value == 0 {
        return Err(format!("{label}必须大于 0"));
    }
    Ok(Some(value))
}

fn parse_dpi(text: &str) -> Result<f32, String> {
    let text = text.trim();
    let value: f32 = text
        .parse()
        .map_err(|_| "分辨率必须是大于 0 的数字".to_string())?;
    if !value.is_finite() || value <= 0.0 {
        return Err("分辨率必须是大于 0 的数字".to_string());
    }
    Ok(value)
}

fn nonempty(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

fn parent_dir(path: &str) -> Option<PathBuf> {
    let path = path.trim();
    if path.is_empty() {
        return None;
    }
    match Path::new(path).parent() {
        Some(dir) if !dir.as_os_str().is_empty() => Some(dir.to_path_buf()),
        Some(_) => Some(PathBuf::from(".")),
        None => None,
    }
}

fn is_pdf(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
}

fn format_key(format: OutputFormat) -> &'static str {
    match format {
        OutputFormat::Png => "png",
        OutputFormat::Jpeg => "jpeg",
        OutputFormat::Webp => "webp",
        OutputFormat::Bmp => "bmp",
    }
}

fn parse_compress_choice(text: &str) -> CompressChoice {
    match text {
        "normal" => CompressChoice::Normal,
        "high" => CompressChoice::HighQuality,
        _ => CompressChoice::MaxSize,
    }
}

fn parse_size_limit(text: &str, unit: SizeUnit) -> Result<u64, String> {
    let value: f64 = text
        .trim()
        .parse()
        .map_err(|_| "体积必须是大于 0 的数字".to_string())?;
    if !value.is_finite() || value <= 0.0 {
        return Err("体积必须是大于 0 的数字".to_string());
    }
    let bytes = match unit {
        SizeUnit::Kb => value * 1024.0,
        SizeUnit::Mb => value * 1024.0 * 1024.0,
    };
    if bytes < 1024.0 {
        return Err("体积上限至少为 1 KB".to_string());
    }
    if bytes > 200.0 * 1024.0 * 1024.0 {
        return Err("体积上限不能超过 200 MB".to_string());
    }
    Ok(bytes.round() as u64)
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.2} MB", bytes as f64 / 1024.0 / 1024.0)
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

fn parse_format(text: &str) -> OutputFormat {
    match text {
        "jpeg" | "jpg" => OutputFormat::Jpeg,
        "webp" => OutputFormat::Webp,
        "bmp" => OutputFormat::Bmp,
        _ => OutputFormat::Png,
    }
}

fn install_cjk_font(ctx: &egui::Context) {
    let candidates = [
        r"C:\Windows\Fonts\Deng.ttf",
        r"C:\Windows\Fonts\simhei.ttf",
        r"C:\Windows\Fonts\msyh.ttc",
    ];
    let Some(bytes) = candidates.iter().find_map(|path| std::fs::read(path).ok()) else {
        return;
    };
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "cjk".to_owned(),
        Arc::new(egui::FontData::from_owned(bytes)),
    );
    if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        family.insert(0, "cjk".to_owned());
    }
    if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
        family.insert(0, "cjk".to_owned());
    }
    ctx.set_fonts(fonts);
}
