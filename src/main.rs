use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, Sender},
    Mutex,
};
use std::thread;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    text::Line,
    widgets::{Block, Borders, Paragraph},
};
use rustfft::{FftPlanner, num_complex::Complex32};
use serde_json::{Value, json};

const SAMPLE_RATE: usize = 22_050;
const FFT_SIZE: usize = 256;
const FFT_HOP_SIZE: usize = FFT_SIZE / 2;
const SPECTRUM_BARS: usize = 48;
const MIN_SPECTRUM_HZ: f32 = 30.0;
const MAX_SPECTRUM_HZ: f32 = 10_000.0;
const SPECTRUM_FPS: u64 = 60;
const NOW_PLAYING_POLL_SECS: u64 = 15;
const MAX_ANALYSIS_BACKLOG_FRAMES: usize = 1;
const SPECTRUM_GAIN: f32 = 0.72;
const CONTROL_SOCKET_PATH: &str = "/tmp/iterm-player.sock";
const STATE_FILE_PATH: &str = "/tmp/iterm-player.json";
const DEFAULT_VOLUME_STEP: u8 = 5;

#[derive(Clone, Copy)]
struct Station {
    key: &'static str,
    label: &'static str,
    stream: &'static str,
}

const STATIONS: [Station; 4] = [
    Station {
        key: "nts1",
        label: "NTS 1",
        stream: "https://stream-relay-geo.ntslive.net/stream?client=direct",
    },
    Station {
        key: "nts2",
        label: "NTS 2",
        stream: "https://stream-relay-geo.ntslive.net/stream2?client=direct",
    },
    Station {
        key: "worldwide",
        label: "Worldwide FM",
        stream: "https://worldwide-fm.radiocult.fm/stream",
    },
    Station {
        key: "fip",
        label: "FIP",
        stream: "https://icecast.radiofrance.fr/fip-hifi.aac?id=radiofrance",
    },
];

struct PlaybackHandles {
    stop_flag: Arc<AtomicBool>,
    worker: JoinHandle<()>,
}

#[derive(Clone, Copy)]
struct Theme {
    name: &'static str,
    color: Color,
}

const THEMES: [Theme; 8] = [
    Theme { name: "cyan", color: Color::Cyan },
    Theme { name: "red", color: Color::Red },
    Theme { name: "yellow", color: Color::Yellow },
    Theme { name: "green", color: Color::Green },
    Theme { name: "blue", color: Color::Blue },
    Theme { name: "pink", color: Color::LightMagenta },
    Theme { name: "magenta", color: Color::Magenta },
    Theme { name: "white", color: Color::White },
];

struct App {
    input: String,
    status: String,
    now_playing: String,
    spectrum: Vec<u16>,
    theme: Theme,
    current_station: Option<Station>,
    playback: Option<PlaybackHandles>,
    spectrum_rx: Option<Receiver<Vec<u16>>>,
    now_playing_rx: Option<Receiver<String>>,
    bitrate_rx: Option<Receiver<u32>>,
    control_rx: Receiver<ControlCommand>,
    shared_state: Arc<Mutex<WidgetState>>,
    volume_step: u8,
    volume_level: Arc<Mutex<f64>>,
    last_spectrum_draw: Instant,
    dirty: bool,
    current_bitrate: Option<u32>,
    fps_frame_count: u32,
    fps_window_start: Instant,
    current_fps: u32,
}

#[derive(Clone)]
struct WidgetState {
    running: bool,
    station_key: Option<String>,
    station_label: Option<String>,
    color: String,
    volume: u8,
    pid: u32,
}

enum ControlCommand {
    Play(String),
    Stop,
    Next,
    Color(String),
    Volume(u8),
}

impl App {
    fn new(control_rx: Receiver<ControlCommand>, shared_state: Arc<Mutex<WidgetState>>) -> Self {
        let volume_step = DEFAULT_VOLUME_STEP;
        Self {
            input: String::new(),
            status: format!(
                "Not playing\nVolume: {volume_step}/10\n\nCommands: /play [station], /next, /color [name], /volume [0-10], /stop, /quit"
            ),
            now_playing: String::new(),
            spectrum: vec![0; SPECTRUM_BARS],
            theme: THEMES[0],
            current_station: None,
            playback: None,
            spectrum_rx: None,
            now_playing_rx: None,
            bitrate_rx: None,
            control_rx,
            shared_state,
            volume_step,
            volume_level: Arc::new(Mutex::new(step_to_volume(volume_step))),
            last_spectrum_draw: Instant::now(),
            dirty: true,
            current_bitrate: None,
            fps_frame_count: 0,
            fps_window_start: Instant::now(),
            current_fps: 0,
        }
    }

    fn set_tab_title(title: &str) {
        print!("\x1b]1;{}\x07", title);
        let _ = io::stdout().flush();
    }

    fn set_idle_status(&mut self) {
        self.status = format!(
            "Not playing\nVolume: {}/10\n\nCommands: /play [station], /next, /color [name], /volume [0-10], /stop, /quit",
            self.volume_step
        );
        Self::set_tab_title("iterm-player");
        self.dirty = true;
        self.sync_shared_state();
    }

    fn set_playing_status(&mut self, station: Station) {
        let mut lines = vec![format!("Playing: {}", station.label)];
        if !self.now_playing.is_empty() {
            lines.push(format!("Now: {}", self.now_playing));
        }
        lines.push(format!("Volume: {}/10", self.volume_step));
        lines.push(format!("Stream: {}", station.stream));
        if self.current_fps > 0 {
            lines.push(format!("FPS: {}", self.current_fps));
        }
        if let Some(kbps) = self.current_bitrate {
            lines.push(format!("Bitrate: {} kbps", kbps));
        }
        lines.push("Commands: /play [station], /next, /color [name], /volume [0-10], /stop, /quit".to_string());
        self.status = lines.join("\n");
        self.dirty = true;
        self.sync_shared_state();
    }

    fn stop_playback(&mut self) {
        if let Some(playback) = self.playback.take() {
            playback.stop_flag.store(true, Ordering::SeqCst);
            let _ = playback.worker.join();
        }

        self.current_station = None;
        self.spectrum_rx = None;
        self.now_playing_rx = None;
        self.bitrate_rx = None;
        self.current_bitrate = None;
        self.now_playing.clear();
        self.spectrum.fill(0);
        self.set_idle_status();
        self.dirty = true;
        self.sync_shared_state();
    }

    fn start_playback(&mut self, station: Station) {
        self.stop_playback();

        let stop_flag = Arc::new(AtomicBool::new(false));
        let (spectrum_tx, spectrum_rx) = mpsc::channel();
        let (now_playing_tx, now_playing_rx) = mpsc::channel();
        let (bitrate_tx, bitrate_rx) = mpsc::channel();
        let worker = spawn_gstreamer_worker(
            station.stream,
            spectrum_tx,
            bitrate_tx,
            Arc::clone(&stop_flag),
            Arc::clone(&self.volume_level),
        );

        spawn_now_playing_worker(station, now_playing_tx, Arc::clone(&stop_flag));

        self.current_station = Some(station);
        self.now_playing.clear();
        self.spectrum.fill(0);
        self.current_bitrate = None;
        self.set_playing_status(station);
        self.dirty = true;
        self.playback = Some(PlaybackHandles {
            stop_flag,
            worker,
        });
        self.spectrum_rx = Some(spectrum_rx);
        self.now_playing_rx = Some(now_playing_rx);
        self.bitrate_rx = Some(bitrate_rx);
        self.sync_shared_state();
    }

    fn process_updates(&mut self) {
        while let Ok(command) = self.control_rx.try_recv() {
            self.execute_control_command(command);
        }

        if let Some(rx) = &self.now_playing_rx {
            let mut latest = None;
            while let Ok(value) = rx.try_recv() {
                latest = Some(value);
            }
            if let Some(now_playing) = latest {
                self.now_playing = now_playing;
                if let Some(station) = self.current_station {
                    self.set_playing_status(station);
                }
                self.dirty = true;
                self.sync_shared_state();
            }
        }

        if let Some(rx) = &self.spectrum_rx {
            let mut latest = None;
            while let Ok(value) = rx.try_recv() {
                latest = Some(value);
            }
            if let Some(spectrum) = latest {
                self.spectrum = spectrum;
                self.dirty = true;
            }
        }

        if let Some(rx) = &self.bitrate_rx {
            let mut latest = None;
            while let Ok(value) = rx.try_recv() {
                latest = Some(value);
            }
            if let Some(bps) = latest {
                let kbps = bps / 1000;
                if self.current_bitrate != Some(kbps) {
                    self.current_bitrate = Some(kbps);
                    if let Some(station) = self.current_station {
                        self.set_playing_status(station);
                    }
                    self.dirty = true;
                }
            }
        }
    }

    fn execute_control_command(&mut self, command: ControlCommand) {
        match command {
            ControlCommand::Play(query) => {
                if let Some(station) = match_station(&query) {
                    self.start_playback(station);
                }
            }
            ControlCommand::Stop => self.stop_playback(),
            ControlCommand::Next => {
                let station = next_station(self.current_station);
                self.start_playback(station);
            }
            ControlCommand::Color(query) => {
                if let Some(theme) = THEMES.iter().copied().find(|theme| {
                    theme.name == query || theme.name.starts_with(&query)
                }) {
                    self.theme = theme;
                    if let Some(station) = self.current_station {
                        self.set_playing_status(station);
                    } else {
                        self.set_idle_status();
                    }
                    self.sync_shared_state();
                }
            }
            ControlCommand::Volume(step) => self.set_volume(step),
        }
    }

    fn set_volume(&mut self, step: u8) {
        self.volume_step = step.min(10);
        if let Ok(mut volume) = self.volume_level.lock() {
            *volume = step_to_volume(self.volume_step);
        }

        if let Some(station) = self.current_station {
            self.set_playing_status(station);
        } else {
            self.set_idle_status();
        }
        self.sync_shared_state();
    }

    fn sync_shared_state(&self) {
        if let Ok(mut state) = self.shared_state.lock() {
            state.running = self.current_station.is_some();
            state.station_key = self.current_station.map(|station| station.key.to_string());
            state.station_label = self.current_station.map(|station| station.label.to_string());
            state.color = self.theme.name.to_string();
            state.volume = self.volume_step;
            state.pid = std::process::id();
            let _ = write_widget_state(&state);
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    configure_gstreamer_env();
    gst::init()?;
    let (control_tx, control_rx) = mpsc::channel();
    let shared_state = Arc::new(Mutex::new(WidgetState {
        running: false,
        station_key: None,
        station_label: None,
        color: THEMES[0].name.to_string(),
        volume: DEFAULT_VOLUME_STEP,
        pid: std::process::id(),
    }));
    let control_server_stop = Arc::new(AtomicBool::new(false));
    let control_server = spawn_control_server(control_tx, Arc::clone(&shared_state), Arc::clone(&control_server_stop));
    let mut app = App::new(control_rx, Arc::clone(&shared_state));
    app.sync_shared_state();
    let mut terminal = init_terminal()?;

    let result = run_app(&mut terminal, &mut app);

    restore_terminal(&mut terminal)?;
    App::set_tab_title("");
    app.stop_playback();
    control_server_stop.store(true, Ordering::SeqCst);
    let _ = UnixStream::connect(CONTROL_SOCKET_PATH);
    let _ = fs::remove_file(CONTROL_SOCKET_PATH);
    let _ = control_server.join();
    result
}

fn configure_gstreamer_env() {
    const HOMEBREW_PREFIX: &str = "/opt/homebrew";
    let lib_dir = format!("{HOMEBREW_PREFIX}/lib");
    let typelib_dir = format!("{lib_dir}/girepository-1.0");
    let plugin_dir = format!("{lib_dir}/gstreamer-1.0");
    let scanner = format!("{HOMEBREW_PREFIX}/Cellar/gstreamer/1.28.1/libexec/gstreamer-1.0/gst-plugin-scanner");
    let gio_modules = format!("{lib_dir}/gio/modules");

    prepend_env_path("DYLD_FALLBACK_LIBRARY_PATH", &lib_dir);
    prepend_env_path("GI_TYPELIB_PATH", &typelib_dir);
    prepend_env_path("GST_PLUGIN_SYSTEM_PATH_1_0", &plugin_dir);
    prepend_env_path("GIO_EXTRA_MODULES", &gio_modules);

    if std::path::Path::new(&scanner).exists() {
        unsafe {
            std::env::set_var("GST_PLUGIN_SCANNER", scanner);
        }
    }
}

fn prepend_env_path(key: &str, value: &str) {
    let existing = std::env::var_os(key);
    let new_value = match existing {
        Some(existing) if !existing.is_empty() => {
            let mut joined = std::ffi::OsString::from(value);
            joined.push(":");
            joined.push(existing);
            joined
        }
        _ => std::ffi::OsString::from(value),
    };

    unsafe {
        std::env::set_var(key, new_value);
    }
}

fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        app.process_updates();

        if app.dirty && app.last_spectrum_draw.elapsed() >= Duration::from_millis(1000 / SPECTRUM_FPS) {
            terminal.draw(|frame| draw_ui(frame, app))?;
            if let Some(station) = app.current_station {
                let anim = spectrum_tab_animation(&app.spectrum);
                let title = if app.now_playing.is_empty() {
                    format!("{} {}", anim, station.label)
                } else {
                    format!("{} {} — {}", anim, station.label, app.now_playing)
                };
                App::set_tab_title(&title);
            }
            app.last_spectrum_draw = Instant::now();
            app.dirty = false;
            app.fps_frame_count += 1;
            let fps_elapsed = app.fps_window_start.elapsed();
            if fps_elapsed >= Duration::from_secs(1) {
                let fps = (app.fps_frame_count as f32 / fps_elapsed.as_secs_f32()).round() as u32;
                if app.current_fps != fps {
                    app.current_fps = fps;
                    if let Some(station) = app.current_station {
                        app.set_playing_status(station);
                    }
                    app.dirty = true;
                }
                app.fps_frame_count = 0;
                app.fps_window_start = Instant::now();
            }
        }

        if event::poll(Duration::from_millis(5))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match key.code {
                    KeyCode::Char('c') if key.modifiers.contains(event::KeyModifiers::CONTROL) => {
                        break;
                    }
                    KeyCode::Char('q') if app.input.is_empty() => break,
                    KeyCode::Tab => {
                        autocomplete_input(app);
                        app.dirty = true;
                    }
                    KeyCode::Enter => {
                        execute_command(app);
                        app.dirty = true;
                    }
                    KeyCode::Backspace => {
                        app.input.pop();
                        app.dirty = true;
                    }
                    KeyCode::Char(ch) => {
                        app.input.push(ch);
                        app.dirty = true;
                    }
                    KeyCode::Esc => {
                        app.input.clear();
                        app.dirty = true;
                    }
                    _ => {}
                }
            }
        }
    }

    Ok(())
}

fn draw_ui(frame: &mut ratatui::Frame<'_>, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(17),
            Constraint::Percentage(58),
            Constraint::Percentage(25),
        ])
        .split(frame.area());

    let status = Paragraph::new(app.status.as_str())
        .block(
            Block::default()
                .title("Status")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(app.theme.color)),
        );
    frame.render_widget(status, chunks[0]);

    let spectrum_text = render_spectrum_text(&app.spectrum, chunks[1].width.saturating_sub(2), chunks[1].height.saturating_sub(2));
    let spectrum = Paragraph::new(spectrum_text)
        .style(Style::default().fg(app.theme.color))
        .block(
            Block::default()
                .title("Spectrum")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(app.theme.color)),
        );
    frame.render_widget(spectrum, chunks[1]);

    let input = Paragraph::new(app.input.as_str())
        .block(
            Block::default()
                .title("Command")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(app.theme.color)),
        );
    frame.render_widget(input, chunks[2]);

    let cursor_x = chunks[2].x + 1 + app.input.chars().count() as u16;
    let cursor_y = chunks[2].y + 1;
    frame.set_cursor_position((cursor_x, cursor_y));
}

fn spectrum_tab_animation(spectrum: &[u16]) -> String {
    const BLOCKS: &[char] = &['▁', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    const SAMPLES: usize = 6;
    let len = spectrum.len();
    if len == 0 {
        return String::new();
    }
    (0..SAMPLES)
        .map(|i| {
            let idx = i * (len - 1) / (SAMPLES - 1);
            let val = spectrum[idx].min(100) as usize;
            BLOCKS[val * (BLOCKS.len() - 1) / 100]
        })
        .collect()
}

fn render_spectrum_text(bins: &[u16], width: u16, height: u16) -> Vec<Line<'static>> {
    let width = width as usize;
    let height = height as usize;

    if width == 0 || height == 0 {
        return Vec::new();
    }

    let display_bins = spread_bins(bins, width);
    let mut rows = vec![vec![' '; width]; height];

    for (col, value) in display_bins.iter().enumerate() {
        let total_units = ((*value as usize) * height * 8) / 100;
        for row in 0..height {
            let units_above_bottom = total_units as isize - (((height - row - 1) * 8) as isize);
            rows[row][col] = vertical_block(units_above_bottom);
        }
    }

    rows.into_iter()
        .map(|row| Line::from(row.into_iter().collect::<String>()))
        .collect()
}

fn spread_bins(bins: &[u16], width: usize) -> Vec<u16> {
    if width == 0 {
        return Vec::new();
    }

    let (slot_count, gap_width) = choose_bar_layout(bins.len(), width);
    let sampled = resample_bins(bins, slot_count.max(1));
    let mut output = vec![0; width];

    if slot_count <= 1 {
        output[width / 2] = sampled[0];
        return output;
    }

    for (i, value) in sampled.into_iter().enumerate() {
        let column = i * (gap_width + 1);
        output[column] = value;
    }

    output
}

fn choose_bar_layout(max_bars: usize, width: usize) -> (usize, usize) {
    let max_bars = max_bars.min(width);

    if max_bars <= 1 {
        return (1, 0);
    }

    for bars in (2..=max_bars).rev() {
        let gaps = bars - 1;
        if gaps == 0 {
            continue;
        }

        let free_columns = width.saturating_sub(bars);
        if free_columns % gaps == 0 {
            return (bars, free_columns / gaps);
        }
    }

    (1, 0)
}

fn resample_bins(bins: &[u16], target_count: usize) -> Vec<u16> {
    if bins.is_empty() {
        return vec![0; target_count];
    }

    if bins.len() <= target_count {
        return bins.to_vec();
    }

    (0..target_count)
        .map(|i| {
            let start = i * bins.len() / target_count;
            let end = ((i + 1) * bins.len() / target_count).max(start + 1);
            let sum: u32 = bins[start..end].iter().map(|&v| v as u32).sum();
            (sum / (end - start) as u32) as u16
        })
        .collect()
}

fn vertical_block(units: isize) -> char {
    match units {
        i if i <= 0 => ' ',
        1 => '▁',
        2 => '▂',
        3 => '▃',
        4 => '▄',
        5 => '▅',
        6 => '▆',
        7 => '▇',
        _ => '█',
    }
}

fn execute_command(app: &mut App) {
    let command = app.input.trim().to_string();
    app.input.clear();

    if command.is_empty() {
        return;
    }

    match command.as_str() {
        "/quit" | "/q" => {
            app.stop_playback();
            std::process::exit(0);
        }
        "/stop" => app.stop_playback(),
        "/next" => {
            let station = next_station(app.current_station);
            app.start_playback(station);
        }
        "/color" => {
            let names = THEMES.iter().map(|theme| theme.name).collect::<Vec<_>>().join(", ");
            app.status = format!("Usage: /color [name]\nAvailable: {names}");
            app.dirty = true;
        }
        "/volume" => {
            app.status = format!("Usage: /volume [0-10]\nCurrent: {}/10", app.volume_step);
            app.dirty = true;
        }
        "/play" => {
            let available = STATIONS.iter().map(|s| s.key).collect::<Vec<_>>().join(", ");
            app.status = format!("Usage: /play [station]\nAvailable: {available}");
            app.dirty = true;
        }
        _ if command.starts_with("/color ") => {
            let query = command.trim_start_matches("/color ").trim().to_lowercase();
            if let Some(theme) = THEMES.iter().copied().find(|theme| {
                theme.name == query || theme.name.starts_with(&query)
            }) {
                app.theme = theme;
                if let Some(station) = app.current_station {
                    app.set_playing_status(station);
                } else {
                    app.set_idle_status();
                }
            } else {
                let names = THEMES.iter().map(|theme| theme.name).collect::<Vec<_>>().join(", ");
                app.status = format!("Unknown color: {query}\nAvailable: {names}");
                app.dirty = true;
            }
        }
        _ if command.starts_with("/play ") => {
            let query = command.trim_start_matches("/play ").trim().to_lowercase();
            if let Some(station) = match_station(&query) {
                app.start_playback(station);
            } else {
                let available = STATIONS.iter().map(|s| s.key).collect::<Vec<_>>().join(", ");
                app.status = format!("Unknown station: {query}\nAvailable: {available}");
            }
        }
        _ if command.starts_with("/volume ") => {
            let query = command.trim_start_matches("/volume ").trim();
            match query.parse::<u8>() {
                Ok(step) if step <= 10 => app.set_volume(step),
                _ => {
                    app.status = format!("Invalid volume: {query}\nUsage: /volume [0-10]");
                    app.dirty = true;
                }
            }
        }
        _ => {
            app.status = "Commands: /play [station], /next, /color [name], /volume [0-10], /stop, /quit".to_string();
            app.dirty = true;
        }
    }
}

fn autocomplete_input(app: &mut App) {
    let trimmed = app.input.trim_start();

    if let Some(rest) = trimmed.strip_prefix("/color ") {
        let query = rest.trim().to_lowercase();
        let theme_names = THEMES.iter().map(|theme| theme.name).collect::<Vec<_>>();
        if let Some(completion) = complete_from_candidates(&query, &theme_names) {
            app.input = format!("/color {completion}");
        }
        return;
    }

    if let Some(rest) = trimmed.strip_prefix("/play ") {
        let query = rest.trim().to_lowercase();
        let station_keys = STATIONS.iter().map(|s| s.key).collect::<Vec<_>>();
        if let Some(completion) = complete_from_candidates(&query, &station_keys) {
            app.input = format!("/play {completion}");
        }
        return;
    }

    let commands = ["/play", "/next", "/stop", "/color", "/volume", "/quit", "/q"];
    let query = trimmed.to_lowercase();
    if let Some(completion) = complete_from_candidates(&query, &commands) {
        app.input = completion;
        if app.input == "/play" || app.input == "/volume" {
            app.input.push(' ');
        }
    }
}

fn complete_from_candidates(query: &str, candidates: &[&str]) -> Option<String> {
    let matches = candidates
        .iter()
        .copied()
        .filter(|candidate| candidate.starts_with(query))
        .collect::<Vec<_>>();

    if matches.is_empty() {
        return None;
    }

    if matches.len() == 1 {
        return Some(matches[0].to_string());
    }

    Some(longest_common_prefix(&matches))
}

fn match_station(query: &str) -> Option<Station> {
    STATIONS.iter().copied().find(|station| {
        station.key == query
            || station.label.to_lowercase() == query
            || station.key.starts_with(query)
            || station.label.to_lowercase().starts_with(query)
    })
}

fn next_station(current: Option<Station>) -> Station {
    if let Some(current) = current {
        let index = STATIONS
            .iter()
            .position(|station| station.key == current.key)
            .unwrap_or(0);
        STATIONS[(index + 1) % STATIONS.len()]
    } else {
        STATIONS[0]
    }
}

fn spawn_control_server(
    tx: Sender<ControlCommand>,
    shared_state: Arc<Mutex<WidgetState>>,
    stop_flag: Arc<AtomicBool>,
) -> JoinHandle<()> {
    let _ = fs::remove_file(CONTROL_SOCKET_PATH);
    thread::spawn(move || {
        let Ok(listener) = UnixListener::bind(CONTROL_SOCKET_PATH) else {
            return;
        };
        let _ = listener.set_nonblocking(true);

        while !stop_flag.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((stream, _)) => {
                    let _ = handle_control_stream(stream, &tx, &shared_state);
                }
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(25));
                }
                Err(_) => break,
            }
        }
    })
}

fn handle_control_stream(
    mut stream: UnixStream,
    tx: &Sender<ControlCommand>,
    shared_state: &Arc<Mutex<WidgetState>>,
) -> io::Result<()> {
    let mut command = String::new();
    {
        let mut reader = BufReader::new(&stream);
        reader.read_line(&mut command)?;
    }
    let command = command.trim();

    let response = if command == "status" {
        if let Ok(state) = shared_state.lock() {
            serde_json::to_string(&json!({
                "running": state.running,
                "station_key": state.station_key,
                "station_label": state.station_label,
                "color": state.color,
                "volume": state.volume,
                "pid": state.pid,
            }))
            .unwrap_or_else(|_| "{\"running\":false}".to_string())
        } else {
            "{\"running\":false}".to_string()
        }
    } else if command == "stop" {
        let _ = tx.send(ControlCommand::Stop);
        "{\"ok\":true}".to_string()
    } else if command == "next" {
        let _ = tx.send(ControlCommand::Next);
        "{\"ok\":true}".to_string()
    } else if let Some(query) = command.strip_prefix("play ") {
        let _ = tx.send(ControlCommand::Play(query.trim().to_lowercase()));
        "{\"ok\":true}".to_string()
    } else if let Some(query) = command.strip_prefix("color ") {
        let _ = tx.send(ControlCommand::Color(query.trim().to_lowercase()));
        "{\"ok\":true}".to_string()
    } else if let Some(query) = command.strip_prefix("volume ") {
        match query.trim().parse::<u8>() {
            Ok(step) if step <= 10 => {
                let _ = tx.send(ControlCommand::Volume(step));
                "{\"ok\":true}".to_string()
            }
            _ => "{\"ok\":false,\"error\":\"invalid volume\"}".to_string(),
        }
    } else {
        "{\"ok\":false,\"error\":\"unknown command\"}".to_string()
    };

    writeln!(stream, "{response}")?;
    Ok(())
}

fn write_widget_state(state: &WidgetState) -> io::Result<()> {
    let data = json!({
        "running": state.running,
        "station_key": state.station_key,
        "station_label": state.station_label,
        "color": state.color,
        "volume": state.volume,
        "pid": state.pid,
    });
    fs::write(STATE_FILE_PATH, serde_json::to_vec_pretty(&data).unwrap_or_default())
}

fn step_to_volume(step: u8) -> f64 {
    (step.min(10) as f64) / 10.0
}

fn longest_common_prefix(candidates: &[&str]) -> String {
    let mut prefix = candidates[0].to_string();

    for candidate in candidates.iter().skip(1) {
        let mut next = String::new();
        for (left, right) in prefix.chars().zip(candidate.chars()) {
            if left != right {
                break;
            }
            next.push(left);
        }
        prefix = next;
        if prefix.is_empty() {
            break;
        }
    }

    prefix
}

fn init_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>, Box<dyn std::error::Error>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    Ok(Terminal::new(backend)?)
}

fn restore_terminal(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
) -> Result<(), Box<dyn std::error::Error>> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

fn spawn_gstreamer_worker(
    url: &'static str,
    tx: Sender<Vec<u16>>,
    bitrate_tx: Sender<u32>,
    stop_flag: Arc<AtomicBool>,
    volume_level: Arc<Mutex<f64>>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let initial_volume = volume_level
            .lock()
            .map(|volume| *volume)
            .unwrap_or(step_to_volume(DEFAULT_VOLUME_STEP));
        let pipeline_description = format!(
            concat!(
                "pipeline name=player ",
                "uridecodebin uri=\"{url}\" name=src ",
                "src. ! queue ! audioconvert ! audioresample ! tee name=split ",
                "split. ! queue ! volume name=player_volume volume={volume} ! autoaudiosink sync=false ",
                "split. ! queue leaky=downstream max-size-buffers=1 max-size-bytes=0 max-size-time=0 ",
                "! audio/x-raw,format=F32LE,channels=1,rate={rate} ",
                "! appsink name=spectrum_sink emit-signals=false sync=false max-buffers=1 drop=true",
            ),
            url = url,
            rate = SAMPLE_RATE,
            volume = initial_volume,
        );

        let Ok(element) = gst::parse::launch(&pipeline_description) else {
            return;
        };
        let Ok(pipeline) = element.downcast::<gst::Pipeline>() else {
            return;
        };
        let Some(appsink_element) = pipeline.by_name("spectrum_sink") else {
            let _ = pipeline.set_state(gst::State::Null);
            return;
        };
        let Some(volume_element) = pipeline.by_name("player_volume") else {
            let _ = pipeline.set_state(gst::State::Null);
            return;
        };
        let Ok(appsink) = appsink_element.downcast::<gst_app::AppSink>() else {
            let _ = pipeline.set_state(gst::State::Null);
            return;
        };
        let mut applied_volume = initial_volume;

        let _ = pipeline.set_state(gst::State::Playing);
        let Some(bus) = pipeline.bus() else {
            let _ = pipeline.set_state(gst::State::Null);
            return;
        };

        let mut analyzer = SpectrumAnalyzer::new();
        let needed = FFT_HOP_SIZE;
        let mut sample_buffer = Vec::<f32>::new();
        let max_backlog = needed * MAX_ANALYSIS_BACKLOG_FRAMES;

        while !stop_flag.load(Ordering::SeqCst) {
            while let Some(message) = bus.timed_pop(gst::ClockTime::from_mseconds(0)) {
                match message.view() {
                    gst::MessageView::Error(_) | gst::MessageView::Eos(_) => {
                        stop_flag.store(true, Ordering::SeqCst);
                        break;
                    }
                    gst::MessageView::Tag(tag_msg) => {
                        let tags = tag_msg.tags();
                        if let Some(bitrate) = tags.get::<gst::tags::Bitrate>() {
                            let _ = bitrate_tx.send(bitrate.get());
                        } else if let Some(bitrate) = tags.get::<gst::tags::NominalBitrate>() {
                            let _ = bitrate_tx.send(bitrate.get());
                        }
                    }
                    _ => {}
                }
            }

            if stop_flag.load(Ordering::SeqCst) {
                break;
            }

            let desired_volume = volume_level.lock().map(|volume| *volume).unwrap_or(applied_volume);
            if (desired_volume - applied_volume).abs() > f64::EPSILON {
                volume_element.set_property("volume", desired_volume);
                applied_volume = desired_volume;
            }

            if let Some(sample) = appsink.try_pull_sample(gst::ClockTime::from_mseconds(10)) {
                let Some(buffer) = sample.buffer() else {
                    continue;
                };
                let Ok(map) = buffer.map_readable() else {
                    continue;
                };

                for chunk in map.as_slice().chunks_exact(4) {
                    sample_buffer.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
                }

                if sample_buffer.len() > max_backlog {
                    let keep_from = sample_buffer.len() - max_backlog;
                    sample_buffer.copy_within(keep_from.., 0);
                    sample_buffer.truncate(max_backlog);
                }

                while sample_buffer.len() >= needed {
                    let spectrum = analyzer.push(&sample_buffer[..needed]);
                    if tx.send(spectrum).is_err() {
                        stop_flag.store(true, Ordering::SeqCst);
                        break;
                    }

                    sample_buffer.copy_within(needed.., 0);
                    sample_buffer.truncate(sample_buffer.len() - needed);
                }
            } else {
                thread::sleep(Duration::from_millis(2));
            }
        }

        let _ = pipeline.set_state(gst::State::Null);
    })
}

fn spawn_now_playing_worker(station: Station, tx: Sender<String>, stop_flag: Arc<AtomicBool>) {
    thread::spawn(move || {
        let client = reqwest::blocking::Client::builder()
            .user_agent("iterm-player")
            .build();

        let Ok(client) = client else {
            return;
        };

        while !stop_flag.load(Ordering::SeqCst) {
            let text = fetch_now_playing(&client, station).unwrap_or_else(|| "Unknown".to_string());
            if tx.send(text).is_err() {
                break;
            }

            for _ in 0..NOW_PLAYING_POLL_SECS {
                if stop_flag.load(Ordering::SeqCst) {
                    return;
                }
                thread::sleep(Duration::from_secs(1));
            }
        }
    });
}

fn fetch_now_playing(client: &reqwest::blocking::Client, station: Station) -> Option<String> {
    match station.key {
        "nts1" | "nts2" => {
            let json: Value = client.get("https://www.nts.live/api/v2/live").send().ok()?.json().ok()?;
            let channel_name = if station.key == "nts1" { "1" } else { "2" };
            let channel = json["results"]
                .as_array()?
                .iter()
                .find(|entry| entry["channel_name"].as_str() == Some(channel_name))?;
            let title = channel["now"]["broadcast_title"].as_str().unwrap_or("Unknown");
            let location = channel["now"]["embeds"]["details"]["location_long"].as_str().unwrap_or("");
            if location.is_empty() {
                Some(title.to_string())
            } else {
                Some(format!("{title} — {location}"))
            }
        }
        "fip" => {
            let json: Value = client
                .get("https://api.radiofrance.fr/livemeta/pull/7")
                .send()
                .ok()?
                .json()
                .ok()?;
            let level = json["levels"].as_array()?.first()?;
            let position = level["position"].as_u64()? as usize;
            let uid = level["items"].as_array()?.get(position)?.as_str()?;
            let step = &json["steps"][uid];
            let title = step["title"].as_str().unwrap_or("Unknown");
            let authors = step["authors"].as_str().unwrap_or("");
            if authors.is_empty() {
                Some(title.to_string())
            } else {
                Some(format!("{title} — {authors}"))
            }
        }
        _ => Some("Unknown".to_string()),
    }
}

struct SpectrumAnalyzer {
    fft: std::sync::Arc<dyn rustfft::Fft<f32>>,
    window: Vec<f32>,
    history: Vec<f32>,
    input: Vec<Complex32>,
    output: Vec<Complex32>,
    band_ranges: Vec<(usize, usize)>,
    smoothed: Vec<f32>,
    dbs: Vec<f32>,
    raw: Vec<f32>,
    spread: Vec<f32>,
    noise_floor_db: f32,
    peak_db: f32,
}

impl SpectrumAnalyzer {
    fn new() -> Self {
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);
        let window = (0..FFT_SIZE)
            .map(|i| 0.5 - 0.5 * ((2.0 * std::f32::consts::PI * i as f32) / (FFT_SIZE as f32 - 1.0)).cos())
            .collect::<Vec<_>>();
        let history = vec![0.0; FFT_SIZE];
        let input = vec![Complex32::new(0.0, 0.0); FFT_SIZE];
        let output = vec![Complex32::new(0.0, 0.0); FFT_SIZE];
        let band_ranges = build_band_ranges();
        let smoothed = vec![0.0; SPECTRUM_BARS];
        let dbs = vec![0.0; SPECTRUM_BARS];
        let raw = vec![0.0; SPECTRUM_BARS];
        let spread = vec![0.0; SPECTRUM_BARS];

        Self {
            fft,
            window,
            history,
            input,
            output,
            band_ranges,
            smoothed,
            dbs,
            raw,
            spread,
            noise_floor_db: -105.0,
            peak_db: -55.0,
        }
    }

    fn push(&mut self, samples: &[f32]) -> Vec<u16> {
        self.history.copy_within(samples.len().., 0);
        self.history[(FFT_SIZE - samples.len())..].copy_from_slice(samples);

        for i in 0..FFT_SIZE {
            self.input[i] = Complex32::new(self.history[i] * self.window[i], 0.0);
        }
        self.output.copy_from_slice(&self.input);
        self.fft.process(&mut self.output);

        let mut frame_min = f32::INFINITY;
        let mut frame_max = f32::NEG_INFINITY;

        for (band, (start_bin, end_bin)) in self.band_ranges.iter().copied().enumerate() {
            let mut energy = 0.0_f32;
            let mut count = 0.0_f32;

            for bin in start_bin..=end_bin {
                let value = self.output[bin];
                energy += value.re * value.re + value.im * value.im;
                count += 1.0;
            }

            let rms = if count > 0.0 {
                (energy / count).sqrt() / FFT_SIZE as f32
            } else {
                0.0
            };
            let db = 20.0 * (rms + 1e-12).log10();
            self.dbs[band] = db;
            frame_min = frame_min.min(db);
            frame_max = frame_max.max(db);
        }

        self.noise_floor_db = (self.noise_floor_db * 0.97) + (frame_min * 0.03);
        self.peak_db = frame_max.max((self.peak_db * 0.92) + (frame_max * 0.08));
        let dynamic_range = (self.peak_db - self.noise_floor_db).max(18.0);

        for (i, db) in self.dbs.iter().copied().enumerate() {
            let normalized = ((db - self.noise_floor_db) / dynamic_range).clamp(0.0, 1.0);
            self.raw[i] = (normalized.powf(0.85) * 100.0 * SPECTRUM_GAIN).clamp(0.0, 100.0);
        }

        smooth_neighbors(&self.raw, &mut self.spread);
        let mut out = vec![0_u16; self.spread.len()];
        for (i, value) in self.spread.iter().copied().enumerate() {
            let prev = self.smoothed[i];
            let alpha = if value > prev { 0.92 } else { 0.58 };
            let next = prev + (value - prev) * alpha;
            self.smoothed[i] = next;
            out[i] = next.round().clamp(0.0, 100.0) as u16;
        }
        out
    }
}

fn build_band_ranges() -> Vec<(usize, usize)> {
    let nyquist = SAMPLE_RATE as f32 / 2.0;
    let max_hz = MAX_SPECTRUM_HZ.min(nyquist - 1.0);
    let min_log = MIN_SPECTRUM_HZ.log10();
    let max_log = max_hz.log10();
    let bin_hz = SAMPLE_RATE as f32 / FFT_SIZE as f32;

    (0..SPECTRUM_BARS)
        .map(|band| {
            let start_hz = 10_f32.powf(min_log + ((max_log - min_log) * band as f32) / SPECTRUM_BARS as f32);
            let end_hz =
                10_f32.powf(min_log + ((max_log - min_log) * (band + 1) as f32) / SPECTRUM_BARS as f32);
            let start_bin = ((start_hz / bin_hz).floor() as usize).max(1);
            let end_bin = ((end_hz / bin_hz).ceil() as usize).max(start_bin + 1).min((FFT_SIZE / 2) - 1);
            (start_bin, end_bin)
        })
        .collect()
}

fn smooth_neighbors(values: &[f32], out: &mut [f32]) {
    for i in 0..values.len() {
        let left = if i > 0 { values[i - 1] } else { values[i] };
        let center = values[i];
        let right = if i + 1 < values.len() { values[i + 1] } else { values[i] };
        out[i] = (left + center * 2.0 + right) / 4.0;
    }
}
