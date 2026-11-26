use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal;
use rustfft::{Fft, FftPlanner, num_complex::Complex};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

const FFT_N: usize = 4096;
const RATE: f32 = 44100.0;
const F_LO: f32 = 50.0;
const F_HI: f32 = 12000.0;
// Calibração das barras: baixe DB_CEIL se ficarem baixas demais, suba se viverem no teto.
const DB_FLOOR: f32 = -60.0;
const DB_CEIL: f32 = -14.0;
const TILT_DB_PER_OCT: f32 = 3.0; // compensa o grave dominante, como o cava faz
const FALL: f32 = 0.045; // quanto a barra cai por quadro
const BLOCKS: [char; 8] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇'];

const HELP: &str = "\
matchyy — mostra a música tocando no terminal: capa, visualizador e progresso

Uso: matchyy [opções]

Opções (todas desligadas por padrão):
  -tc, --terminal-color    usa a cor do tema do terminal em vez da cor da capa
  -p,  --player <nome>     segue só esse player (ex.: spotify, mpv; veja `playerctl -l`)
  -m,  --monitor <fonte>   captura o áudio dessa fonte em vez de @DEFAULT_MONITOR@
  -f,  --fps <n>           quadros por segundo (padrão 30)
  -j,  --json              imprime a música atual em JSON e sai (para waybar, eww, scripts)
  -mr, --mirror            espelha o espectro: graves no centro, agudos nas pontas
  -sn, --sensitivity <x>   multiplica a altura das barras (ex.: 1.5 ou 0.7)
  -gv, --gravity <x>       velocidade de queda das barras (padrão 1.0; maior cai mais rápido)
  -pk, --peaks             marca o pico de cada barra, que cai devagar
  -i,  --invert            barras crescem de cima para baixo
  -d,  --dense             barras de 1 coluna, sem espaço entre elas
  -h,  --help              mostra esta ajuda
  -V,  --version           mostra a versão

Teclas: espaço play/pause · n/→ próxima · p/← anterior · q/Esc sai
";

#[derive(Default)]
struct Opts {
    terminal_color: bool,
    player: Option<String>,
    monitor: Option<String>,
    fps: Option<u32>,
    json: bool,
    mirror: bool,
    sensitivity: Option<f32>,
    gravity: Option<f32>,
    peaks: bool,
    invert: bool,
    dense: bool,
}

static OPTS: OnceLock<Opts> = OnceLock::new();

fn opts() -> &'static Opts {
    OPTS.get_or_init(Opts::default)
}

fn num<T: std::str::FromStr + PartialOrd + Default>(flag: &str, v: String) -> Result<T, String> {
    v.parse()
        .ok()
        .filter(|n| *n > T::default())
        .ok_or(format!("valor inválido para {flag}: {v}"))
}

fn parse_args(mut it: impl Iterator<Item = String>) -> Result<Opts, String> {
    let mut o = Opts::default();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .ok_or(format!("{a} precisa de um valor (veja --help)"))
        };
        match a.as_str() {
            "-tc" | "--terminal-color" => o.terminal_color = true,
            "-p" | "--player" => o.player = Some(val()?),
            "-m" | "--monitor" => o.monitor = Some(val()?),
            "-f" | "--fps" => o.fps = Some(num(&a, val()?)?),
            "-j" | "--json" => o.json = true,
            "-mr" | "--mirror" => o.mirror = true,
            "-sn" | "--sensitivity" => o.sensitivity = Some(num(&a, val()?)?),
            "-gv" | "--gravity" => o.gravity = Some(num(&a, val()?)?),
            "-pk" | "--peaks" => o.peaks = true,
            "-i" | "--invert" => o.invert = true,
            "-d" | "--dense" => o.dense = true,
            "-h" | "--help" => {
                print!("{HELP}");
                std::process::exit(0);
            }
            "-V" | "--version" => {
                println!("matchyy {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            _ => return Err(format!("opção desconhecida: {a} (veja --help)")),
        }
    }
    Ok(o)
}

/// playerctl já apontado para o player escolhido em --player.
fn pctl() -> Command {
    let mut c = Command::new("playerctl");
    if let Some(p) = &opts().player {
        c.args(["-p", p]);
    }
    c.stderr(Stdio::null());
    c
}

fn json_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn print_json(t: &Track) {
    println!(
        "{{\"status\":{},\"title\":{},\"artist\":{},\"album\":{},\"position\":{:.1},\"length\":{:.1},\"art_url\":{}}}",
        json_str(&t.status),
        json_str(&t.title),
        json_str(&t.artist),
        json_str(&t.album),
        t.pos_us / 1e6,
        t.len_us / 1e6,
        json_str(&t.art_url)
    );
}

#[derive(Default, Clone)]
struct Track {
    status: String,
    pos_us: f64,
    len_us: f64,
    title: String,
    artist: String,
    album: String,
    art_url: String,
    at: Option<Instant>,
}

struct Cover {
    png: Vec<u8>,
    color: (u8, u8, u8),
}

#[derive(Default)]
struct Shared {
    track: Track,
    cover: Option<Cover>,
    cover_gen: u64,
}

fn read_track() -> Track {
    let fmt = "{{status}}\t{{position}}\t{{mpris:length}}\t{{title}}\t{{artist}}\t{{album}}\t{{mpris:artUrl}}";
    let Ok(o) = pctl().args(["metadata", "--format", fmt]).output() else {
        return Track::default();
    };
    let s = String::from_utf8_lossy(&o.stdout);
    let f: Vec<&str> = s.trim_end_matches('\n').split('\t').collect();
    if !o.status.success() || f.len() < 7 {
        return Track::default();
    }
    Track {
        status: f[0].into(),
        pos_us: f[1].parse().unwrap_or(0.0),
        len_us: f[2].parse().unwrap_or(0.0),
        title: f[3].into(),
        artist: f[4].into(),
        album: f[5].into(),
        art_url: f[6].into(),
        at: Some(Instant::now()),
    }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = (b[i] == b'%' && i + 2 < b.len())
            .then(|| std::str::from_utf8(&b[i + 1..i + 3]).ok())
            .flatten()
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match hex {
            Some(v) => {
                out.push(v);
                i += 3;
            }
            None => {
                out.push(b[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into()
}

fn load_cover(url: &str) -> Option<Cover> {
    let bytes = if let Some(p) = url.strip_prefix("file://") {
        std::fs::read(percent_decode(p)).ok()?
    } else if url.starts_with("http") {
        let o = Command::new("curl")
            .args(["-sfL", "--max-time", "8", url])
            .output()
            .ok()?;
        o.status.success().then_some(o.stdout)?
    } else {
        return None;
    };
    let img = image::load_from_memory(&bytes).ok()?.resize_to_fill(
        400,
        400,
        image::imageops::FilterType::Triangle,
    );
    // Cor de destaque: média da capa, mais saturada e clara.
    let rgb = img.to_rgb8();
    let n = (rgb.width() * rgb.height()) as f32;
    let mut avg = [0f32; 3];
    for p in rgb.pixels() {
        for c in 0..3 {
            avg[c] += p[c] as f32 / n;
        }
    }
    let mean = (avg[0] + avg[1] + avg[2]) / 3.0;
    let sat = avg.map(|c| (mean + (c - mean) * 1.8).max(0.0));
    let k = 235.0 / sat.iter().cloned().fold(1.0, f32::max);
    let color = ((sat[0] * k) as u8, (sat[1] * k) as u8, (sat[2] * k) as u8);
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .ok()?;
    Some(Cover { png, color })
}

fn poll_player(shared: Arc<Mutex<Shared>>) {
    let mut last_url = None::<String>;
    loop {
        let t = read_track();
        if last_url.as_deref() != Some(&t.art_url) {
            last_url = Some(t.art_url.clone());
            let cover = load_cover(&t.art_url);
            let mut s = shared.lock().unwrap();
            s.cover = cover;
            s.cover_gen += 1;
        }
        shared.lock().unwrap().track = t;
        thread::sleep(Duration::from_millis(400));
    }
}

fn spawn_audio(buf: Arc<Mutex<VecDeque<f32>>>) -> Option<Child> {
    let monitor = opts().monitor.as_deref().unwrap_or("@DEFAULT_MONITOR@");
    let mut child = Command::new("parec")
        .args([
            "-d",
            monitor,
            "--format=s16le",
            "--rate=44100",
            "--channels=1",
            "--raw",
            "--latency-msec=20",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut out = child.stdout.take()?;
    thread::spawn(move || {
        let mut b = [0u8; 1024];
        while out.read_exact(&mut b).is_ok() {
            let mut q = buf.lock().unwrap();
            q.extend(
                b.as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| i16::from_le_bytes(*c) as f32 / 32768.0),
            );
            while q.len() > FFT_N {
                q.pop_front();
            }
        }
    });
    Some(child)
}

/// Níveis 0..1 de `n` faixas em escala logarítmica entre F_LO e F_HI.
fn bands(fft: &dyn Fft<f32>, window: &[f32], samples: &[f32], n: usize) -> Vec<f32> {
    let mut data = vec![Complex::new(0.0, 0.0); FFT_N];
    for (i, (s, w)) in samples.iter().zip(window).enumerate() {
        data[i].re = s * w;
    }
    fft.process(&mut data);
    let bin_hz = RATE / FFT_N as f32;
    let gain = opts().sensitivity.unwrap_or(1.0) / (FFT_N as f32 / 4.0);
    let edge = |i: usize| F_LO * (F_HI / F_LO).powf(i as f32 / n as f32);
    (0..n)
        .map(|i| {
            let (f0, f1) = (edge(i), edge(i + 1));
            let lo = (f0 / bin_hz) as usize;
            let hi = ((f1 / bin_hz) as usize).max(lo + 1);
            let mag = data[lo..hi].iter().map(|c| c.norm()).fold(0.0, f32::max) * gain;
            let db =
                20.0 * mag.max(1e-9).log10() + TILT_DB_PER_OCT * ((f0 * f1).sqrt() / 1000.0).log2();
            ((db - DB_FLOOR) / (DB_CEIL - DB_FLOOR)).clamp(0.0, 1.0)
        })
        .collect()
}

fn base64(d: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(d.len() * 4 / 3 + 4);
    for c in d.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            s.push(if i <= c.len() {
                T[(n >> (18 - 6 * i) & 63) as usize] as char
            } else {
                '='
            });
        }
    }
    s
}

/// Protocolo gráfico do kitty (o mesmo que o fastfetch usa no kitty).
fn kitty_image(out: &mut Vec<u8>, png: &[u8], cols: u16, rows: u16) {
    let b64 = base64(png);
    let chunks: Vec<&[u8]> = b64.as_bytes().chunks(4096).collect();
    for (i, c) in chunks.iter().enumerate() {
        let m = (i + 1 < chunks.len()) as u8;
        if i == 0 {
            let _ = write!(out, "\x1b_Ga=T,f=100,i=1,q=2,C=1,c={cols},r={rows},m={m};");
        } else {
            let _ = write!(out, "\x1b_Gm={m};");
        }
        out.extend_from_slice(c);
        out.extend_from_slice(b"\x1b\\");
    }
}

fn fmt_time(us: f64) -> String {
    let s = (us.max(0.0) / 1e6) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

fn cut(s: &str, w: usize) -> String {
    if s.chars().count() <= w {
        s.into()
    } else {
        s.chars().take(w.saturating_sub(1)).chain(['…']).collect()
    }
}

struct Layout {
    cols: u16,
    x: u16,
    y: u16,
    cw: u16,
    ch: u16,
    rx: u16,
    rw: u16,
}

fn layout() -> std::io::Result<Layout> {
    let ws = terminal::window_size()?;
    let (cols, rows) = (ws.columns, ws.rows);
    let ratio = if ws.width > 0 && ws.height > 0 {
        (ws.height as f32 / rows as f32) / (ws.width as f32 / cols as f32)
    } else {
        2.0
    };
    let mut ch = rows.saturating_sub(6).clamp(3, 18);
    let mut cw = (ch as f32 * ratio).round() as u16;
    let max_cw = cols * 2 / 5;
    if cw > max_cw {
        cw = max_cw;
        ch = ((cw as f32 / ratio).round() as u16).max(1);
    }
    let x = 3;
    let rx = x + cw + 4;
    Ok(Layout {
        cols,
        x,
        y: rows.saturating_sub(ch + 2) / 2 + 1,
        cw,
        ch,
        rx,
        rw: cols.saturating_sub(rx + 2),
    })
}

fn playerctl(cmd: &str) {
    let _ = pctl().arg(cmd).status();
}

fn run(shared: &Mutex<Shared>, audio: &Mutex<VecDeque<f32>>) -> std::io::Result<()> {
    let fft = FftPlanner::new().plan_fft_forward(FFT_N);
    let window: Vec<f32> = (0..FFT_N)
        .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (FFT_N - 1) as f32).cos())
        .collect();
    let mut stdout = std::io::stdout();
    let mut lay: Option<Layout> = None;
    let mut seen_gen = u64::MAX;
    let mut accent = (64u8, 224u8, 255u8);
    let mut bars: Vec<f32> = Vec::new();
    let mut peaks: Vec<f32> = Vec::new();
    let o = opts();
    let fps = o.fps.unwrap_or(30);
    let per_frame = 30.0 / fps as f32; // quedas calibradas para 30 fps
    let fall = FALL * o.gravity.unwrap_or(1.0) * per_frame;
    // Cor de destaque: RGB da capa, ou ciano do tema com --terminal-color.
    let paint = |(r, g, b): (u8, u8, u8), k: f32| {
        if o.terminal_color {
            "\x1b[36m".to_string()
        } else {
            format!(
                "\x1b[38;2;{};{};{}m",
                (r as f32 * k) as u8,
                (g as f32 * k) as u8,
                (b as f32 * k) as u8
            )
        }
    };
    loop {
        if event::poll(Duration::from_millis(1000 / fps as u64))? {
            match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => match k.code {
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                    KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                        return Ok(());
                    }
                    KeyCode::Char(' ') => playerctl("play-pause"),
                    KeyCode::Char('n') | KeyCode::Right => playerctl("next"),
                    KeyCode::Char('p') | KeyCode::Left => playerctl("previous"),
                    _ => {}
                },
                Event::Resize(..) => lay = None,
                _ => {}
            }
        }

        let mut f: Vec<u8> = b"\x1b[?2026h".to_vec();
        let fresh = lay.is_none();
        if fresh {
            lay = Some(layout()?);
            f.extend_from_slice(b"\x1b[2J");
        }
        let l = lay.as_ref().unwrap();

        let track = {
            let s = shared.lock().unwrap();
            if fresh || s.cover_gen != seen_gen {
                seen_gen = s.cover_gen;
                f.extend_from_slice(b"\x1b_Ga=d,d=A,q=2\x1b\\");
                for r in 0..l.ch {
                    let _ = write!(f, "\x1b[{};{}H{}", l.y + r, l.x, " ".repeat(l.cw as usize));
                }
                match &s.cover {
                    Some(c) => {
                        accent = c.color;
                        let _ = write!(f, "\x1b[{};{}H", l.y, l.x);
                        kitty_image(&mut f, &c.png, l.cw, l.ch);
                    }
                    None => {
                        let _ = write!(
                            f,
                            "\x1b[{};{}H\x1b[2m♪\x1b[0m",
                            l.y + l.ch / 2,
                            l.x + l.cw / 2
                        );
                    }
                }
            }
            s.track.clone()
        };
        let color = paint(accent, 1.0);
        let w = l.rw as usize;

        // Título, artista e álbum ao lado da capa.
        let lines = if track.at.is_none() {
            [
                "\x1b[2mNada tocando".to_string(),
                String::new(),
                String::new(),
            ]
        } else {
            [
                format!("\x1b[1m{}", cut(&track.title, w)),
                format!("{color}{}", cut(&track.artist, w)),
                format!(
                    "\x1b[2m{}",
                    if track.album == track.title {
                        String::new()
                    } else {
                        cut(&track.album, w)
                    }
                ),
            ]
        };
        for (i, t) in lines.iter().enumerate() {
            let _ = write!(f, "\x1b[{};{}H{t}\x1b[0m\x1b[K", l.y + i as u16, l.rx);
        }

        // Barras do visualizador, sem fundo.
        let bar_h = l.ch.saturating_sub(4).max(1) as usize;
        let nb = if o.dense { w } else { (w + 1) / 3 };
        bars.resize(nb, 0.0);
        peaks.resize(nb, 0.0);
        let samples: Vec<f32> = audio.lock().unwrap().iter().copied().collect();
        let levels = if o.mirror {
            let half = bands(&*fft, &window, &samples, nb.div_ceil(2));
            half.iter()
                .rev()
                .chain(half.iter().skip(nb % 2))
                .copied()
                .collect()
        } else {
            bands(&*fft, &window, &samples, nb)
        };
        for ((cur, pk), v) in bars.iter_mut().zip(peaks.iter_mut()).zip(levels) {
            *cur = if v > *cur {
                *cur + (v - *cur) * 0.7
            } else {
                (*cur - fall).max(v)
            };
            *pk = (*pk - 0.008 * per_frame).max(*cur);
        }
        let top = l.y + l.ch - bar_h as u16;
        for row in 0..bar_h {
            // Distância (em linhas) da base de onde as barras nascem.
            let base = if o.invert { row } else { bar_h - 1 - row };
            let k = 0.55 + 0.45 * base as f32 / bar_h.max(2).saturating_sub(1) as f32;
            let _ = write!(f, "\x1b[{};{}H{}", top + row as u16, l.rx, paint(accent, k));
            let mut line = String::new();
            for (v, pk) in bars.iter().zip(&peaks) {
                let rem = (v * bar_h as f32 * 8.0).round() as isize - base as isize * 8;
                let mut c = match rem {
                    8.. => '█',
                    ..=0 => ' ',
                    _ if o.invert => {
                        if rem >= 4 {
                            '▀'
                        } else {
                            ' '
                        }
                    }
                    _ => BLOCKS[rem as usize],
                };
                if o.peaks && c == ' ' && *pk > 0.02 && (pk * bar_h as f32) as usize == base {
                    c = if o.invert { '▔' } else { '▁' };
                }
                line.push(c);
                if !o.dense {
                    line.push(c);
                    line.push(' ');
                }
            }
            let _ = write!(f, "{}\x1b[0m\x1b[K", line.trim_end());
        }

        // Barra de progresso embaixo: tempo atual à esquerda, duração à direita.
        let mut pos = track.pos_us;
        if track.status == "Playing" {
            pos += track.at.map_or(0.0, |t| t.elapsed().as_micros() as f64);
        }
        let len = track.len_us;
        let pos = if len > 0.0 { pos.min(len) } else { pos };
        let (left, right) = (fmt_time(pos), fmt_time(len));
        let bw = (l.cols as usize).saturating_sub(l.x as usize + 2 + left.len() + right.len() + 2);
        let filled = if len > 0.0 {
            ((pos / len) * bw as f64) as usize
        } else {
            0
        }
        .min(bw.saturating_sub(1));
        let _ = write!(
            f,
            "\x1b[{};{}H{left} {color}{}●\x1b[38;5;239m{}\x1b[0m {right}\x1b[K",
            l.y + l.ch + 1,
            l.x,
            "━".repeat(filled),
            "─".repeat(bw.saturating_sub(filled + 1)),
        );

        f.extend_from_slice(b"\x1b[?2026l");
        stdout.write_all(&f)?;
        stdout.flush()?;
    }
}

fn main() -> std::io::Result<()> {
    match parse_args(std::env::args().skip(1)) {
        Ok(o) => {
            let _ = OPTS.set(o);
        }
        Err(e) => {
            eprintln!("matchyy: {e}");
            std::process::exit(2);
        }
    }
    if opts().json {
        print_json(&read_track());
        return Ok(());
    }
    let shared = Arc::new(Mutex::new(Shared::default()));
    let s = shared.clone();
    thread::spawn(move || poll_player(s));
    let audio = Arc::new(Mutex::new(VecDeque::with_capacity(FFT_N + 512)));
    let mut parec = spawn_audio(audio.clone());

    terminal::enable_raw_mode()?;
    print!("\x1b[?1049h\x1b[?25l");
    let res = run(&shared, &audio);
    print!("\x1b_Ga=d,d=A,q=2\x1b\\\x1b[?25h\x1b[?1049l");
    std::io::stdout().flush()?;
    terminal::disable_raw_mode()?;
    if let Some(c) = parec.as_mut() {
        let _ = c.kill();
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn senoide_cai_na_faixa_certa() {
        let fft = FftPlanner::new().plan_fft_forward(FFT_N);
        let window: Vec<f32> = (0..FFT_N)
            .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (FFT_N - 1) as f32).cos())
            .collect();
        let sine: Vec<f32> = (0..FFT_N)
            .map(|i| 0.5 * (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / RATE).sin())
            .collect();
        let b = bands(&*fft, &window, &sine, 30);
        let peak = b
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0;
        let edge = |i: usize| F_LO * (F_HI / F_LO).powf(i as f32 / 30.0);
        assert!(
            edge(peak) <= 1000.0 && 1000.0 < edge(peak + 1),
            "pico na faixa {peak}"
        );
        assert!(
            bands(&*fft, &window, &vec![0.0; FFT_N], 30)
                .iter()
                .all(|&v| v == 0.0)
        );
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(percent_decode("/a%20b"), "/a b");
        assert_eq!(json_str("a\"b\\\n"), "\"a\\\"b\\\\\\u000a\"");
    }

    #[test]
    fn opcoes() {
        let args = |s: &str| parse_args(s.split_whitespace().map(String::from));
        let d = args("").unwrap();
        assert!(!d.terminal_color && !d.mirror && !d.peaks && !d.json && d.player.is_none());
        let o = args("-tc --player spotify -sn 1.5 -f 60 -mr -pk -i -d").unwrap();
        assert!(o.terminal_color && o.mirror && o.peaks && o.invert && o.dense);
        assert_eq!(
            (o.player.as_deref(), o.sensitivity, o.fps),
            (Some("spotify"), Some(1.5), Some(60))
        );
        assert!(args("--fps 0").is_err() && args("-p").is_err() && args("--nada").is_err());
    }
}
