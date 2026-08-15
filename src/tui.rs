#[cfg(feature = "tui")]
use {
    anyhow::Result,
    crossterm::{
        ExecutableCommand,
        event::{self, Event, KeyCode, KeyEventKind},
        terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
    },
    ratatui::{
        Frame, Terminal,
        backend::CrosstermBackend,
        layout::{Alignment, Constraint, Direction, Layout, Rect},
        style::{Color, Modifier, Style, Stylize},
        text::{Line, Span, Text},
        widgets::{BarChart, Block, Borders, Gauge, Paragraph, Row, Sparkline, Table},
    },
    std::{
        io,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::{Duration, Instant},
    },
    tokio::sync::RwLock,
};

#[cfg(feature = "tui")]
use crate::fmt::group;
#[cfg(feature = "tui")]
use crate::stats::{Report, Stats};

#[cfg(feature = "tui")]
#[derive(Clone)]
pub struct TuiState {
    pub report: Arc<RwLock<Report>>,
    pub start_time: Instant,
    pub url: String,
    pub concurrency: u32,
    pub duration: Duration,
    pause: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
}

#[cfg(feature = "tui")]
impl TuiState {
    pub fn new(url: String, concurrency: u32, duration: Duration) -> Self {
        Self {
            report: Arc::new(RwLock::new(Report::new(url.clone()))),
            start_time: Instant::now(),
            url,
            concurrency,
            duration,
            pause: Arc::new(AtomicBool::new(false)),
            stop: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn report(&self) -> Arc<RwLock<Report>> {
        self.report.clone()
    }

    pub fn stop_flag(&self) -> Arc<AtomicBool> {
        self.stop.clone()
    }

    pub fn pause_flag(&self) -> Arc<AtomicBool> {
        self.pause.clone()
    }
}

#[cfg(feature = "tui")]
pub fn run_tui(state: TuiState) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    stdout.execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let tick_rate = Duration::from_millis(250);
    let mut last_tick = Instant::now();

    let mut rates: Vec<f64> = Vec::new();
    let mut last_requests = 0u64;
    let mut last_sample = Instant::now();

    loop {
        if state.stop.load(Ordering::Relaxed) {
            break;
        }
        terminal.draw(|f| draw_ui(f, &state, &rates))?;

        // Sample the request counter each tick to feed the req/s sparkline.
        let reqs = state
            .report
            .try_read()
            .map(|r| r.requests)
            .unwrap_or(last_requests);
        let now = Instant::now();
        let dt = now.duration_since(last_sample).as_secs_f64();
        if dt >= 0.2 {
            rates.push((reqs - last_requests) as f64 / dt);
            if rates.len() > 60 {
                rates.remove(0);
            }
            last_requests = reqs;
            last_sample = now;
        }

        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        if event::poll(timeout)?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Char('p') => {
                    state.pause.fetch_xor(true, Ordering::Relaxed);
                }
                _ => {}
            }
        }

        if last_tick.elapsed() >= tick_rate {
            last_tick = Instant::now();
        }
    }

    disable_raw_mode()?;
    terminal.backend_mut().execute(LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

#[cfg(feature = "tui")]
fn draw_ui(f: &mut Frame, state: &TuiState, rates: &[f64]) {
    let report_guard = state.report.try_read();
    let report = match report_guard {
        Ok(r) => r,
        Err(_) => return,
    };

    let stats = report.stats();
    let elapsed = state.start_time.elapsed().as_secs_f64();
    let paused = state.pause.load(Ordering::Relaxed);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4), // Header
            Constraint::Length(6), // Stats grid
            Constraint::Length(4), // Sparkline + errors
            Constraint::Length(3), // Status codes
            Constraint::Min(8),    // Chart
            Constraint::Length(6), // Percentiles table
            Constraint::Length(2), // Footer
        ])
        .split(f.area());

    draw_header(f, chunks[0], state, elapsed);
    draw_stats_grid(f, chunks[1], &stats, elapsed);
    draw_mid(f, chunks[2], &report, rates);
    draw_status(f, chunks[3], &report);
    draw_histogram(f, chunks[4], &stats);
    draw_percentiles(f, chunks[5], &stats);
    draw_footer(f, chunks[6], paused);
}

#[cfg(feature = "tui")]
fn draw_header(f: &mut Frame, area: Rect, state: &TuiState, elapsed: f64) {
    let total = state.duration.as_secs_f64();
    let title = Line::from(vec![
        " auger ".bold().on_blue(),
        format!(" — {} ", state.url).blue(),
        format!("({:.0}/{:.0}s) ", elapsed, total).dark_gray(),
        format!("{} workers ", state.concurrency).green(),
    ]);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().blue());
    let inner = block.inner(area);
    f.render_widget(block, area);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(inner);
    f.render_widget(Paragraph::new(title), rows[0]);
    let pct = if total > 0.0 {
        ((elapsed / total) * 100.0).clamp(0.0, 100.0) as u16
    } else {
        0
    };
    let gauge = Gauge::default()
        .gauge_style(Style::default().fg(Color::Cyan))
        .percent(pct);
    f.render_widget(gauge, rows[1]);
}

#[cfg(feature = "tui")]
fn draw_stats_grid(f: &mut Frame, area: Rect, stats: &Stats, _elapsed: f64) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(20),
            Constraint::Percentage(20),
            Constraint::Percentage(20),
            Constraint::Percentage(20),
            Constraint::Percentage(20),
        ])
        .split(area);

    let stat_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let label_style = Style::default().fg(Color::DarkGray);

    let stats_data = [
        ("Req/s", format!("{:.0}", stats.rps)),
        ("Mean", format!("{:.1} ms", stats.mean_ms)),
        ("p50", format!("{:.1} ms", stats.p50)),
        ("p95", format!("{:.1} ms", stats.p95)),
        ("p99", format!("{:.1} ms", stats.p99)),
    ];

    for (i, (label, value)) in stats_data.iter().enumerate() {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::new().dark_gray());
        let text = Text::from(vec![
            Line::from(vec![Span::styled(*label, label_style)]),
            Line::from(vec![Span::styled(value.as_str(), stat_style)]),
        ]);
        let paragraph = Paragraph::new(text)
            .block(block)
            .alignment(Alignment::Center);
        f.render_widget(paragraph, chunks[i]);
    }
}

#[cfg(feature = "tui")]
fn draw_mid(f: &mut Frame, area: Rect, report: &Report, rates: &[f64]) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);
    draw_sparkline(f, chunks[0], rates);
    draw_errors(f, chunks[1], report);
}

#[cfg(feature = "tui")]
fn draw_sparkline(f: &mut Frame, area: Rect, rates: &[f64]) {
    let data: Vec<u64> = rates.iter().map(|r| r.round().max(0.0) as u64).collect();
    let spark = Sparkline::default()
        .block(Block::default().title(" req/s ").borders(Borders::ALL))
        .data(&data)
        .style(Style::default().fg(Color::Cyan));
    f.render_widget(spark, area);
}

#[cfg(feature = "tui")]
fn draw_errors(f: &mut Frame, area: Rect, report: &Report) {
    let mut items = vec![format!("total {}", group(report.errors))];
    if report.errors_timeout > 0 {
        items.push(format!("timeout {}", group(report.errors_timeout)));
    }
    if report.errors_connect > 0 {
        items.push(format!("conn {}", group(report.errors_connect)));
    }
    if report.errors_tls > 0 {
        items.push(format!("tls {}", group(report.errors_tls)));
    }
    if report.errors_status > 0 {
        items.push(format!("status {}", group(report.errors_status)));
    }
    if report.errors_other > 0 {
        items.push(format!("other {}", group(report.errors_other)));
    }
    let text = Line::from(items.join("  "));
    let block = Block::default().title(" Errors ").borders(Borders::ALL);
    let paragraph = Paragraph::new(text)
        .block(block)
        .alignment(Alignment::Center);
    f.render_widget(paragraph, area);
}

#[cfg(feature = "tui")]
fn draw_status(f: &mut Frame, area: Rect, report: &Report) {
    let block = Block::default().title(" Status ").borders(Borders::ALL);
    if report.statuses.is_empty() {
        let paragraph = Paragraph::new("Waiting for responses...")
            .block(block)
            .alignment(Alignment::Center);
        f.render_widget(paragraph, area);
        return;
    }
    let mut spans: Vec<Span> = Vec::new();
    for (i, (code, count)) in report.statuses.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        let color = match code {
            200..=299 => Color::Green,
            300..=399 => Color::Yellow,
            400..=499 => Color::Red,
            _ => Color::Magenta,
        };
        spans.push(Span::styled(
            format!("{} x{}", code, group(*count)),
            Style::default().fg(color),
        ));
    }
    let paragraph = Paragraph::new(Line::from(spans))
        .block(block)
        .alignment(Alignment::Left);
    f.render_widget(paragraph, area);
}

#[cfg(feature = "tui")]
fn draw_histogram(f: &mut Frame, area: Rect, stats: &Stats) {
    if stats.histogram.is_empty() {
        let paragraph = Paragraph::new("Collecting data...").alignment(Alignment::Center);
        f.render_widget(paragraph, area);
        return;
    }

    let labels: Vec<String> = stats
        .histogram
        .iter()
        .map(|b| format!("{:.0}-{:.0}", b.lo, b.hi))
        .collect();
    let data: Vec<(&str, u64)> = labels
        .iter()
        .zip(stats.histogram.iter().map(|b| b.count))
        .map(|(label, count)| (label.as_str(), count))
        .collect();

    let max_count = stats.histogram.iter().map(|b| b.count).max().unwrap_or(1);

    let barchart = BarChart::default()
        .block(
            Block::default()
                .title(" Latency Histogram (ms) ")
                .borders(Borders::ALL),
        )
        .data(&data)
        .bar_width(4)
        .bar_gap(1)
        .bar_style(Style::default().fg(Color::Cyan))
        .value_style(Style::default().fg(Color::DarkGray))
        .label_style(Style::default().fg(Color::DarkGray))
        .max(max_count);

    f.render_widget(barchart, area);
}

#[cfg(feature = "tui")]
fn draw_percentiles(f: &mut Frame, area: Rect, stats: &Stats) {
    let rows = vec![
        Row::new(vec!["p50", "p75", "p90", "p95", "p99", "Max"]).style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Row::new(vec![
            format!("{:.1} ms", stats.p50),
            format!("{:.1} ms", stats.p75),
            format!("{:.1} ms", stats.p90),
            format!("{:.1} ms", stats.p95),
            format!("{:.1} ms", stats.p99),
            format!("{:.1} ms", stats.max_ms),
        ]),
    ];

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(16),
            Constraint::Percentage(16),
            Constraint::Percentage(16),
            Constraint::Percentage(16),
            Constraint::Percentage(16),
            Constraint::Percentage(20),
        ],
    )
    .block(
        Block::default()
            .title(" Percentiles (ms) ")
            .borders(Borders::ALL),
    )
    .column_spacing(2);

    f.render_widget(table, area);
}

#[cfg(feature = "tui")]
fn draw_footer(f: &mut Frame, area: Rect, paused: bool) {
    let text = if paused {
        Line::from(vec![
            " PAUSED ".bold().red(),
            " press p to resume ".dark_gray(),
        ])
    } else {
        Line::from(vec![
            " Press ".dark_gray(),
            "q".bold().yellow(),
            "/".dark_gray(),
            "Esc".bold().yellow(),
            " to quit · ".dark_gray(),
            "p".bold().yellow(),
            " to pause ".dark_gray(),
        ])
    };
    let paragraph = Paragraph::new(text).alignment(Alignment::Center);
    f.render_widget(paragraph, area);
}

// Non-TUI stubs for when feature is disabled
#[cfg(not(feature = "tui"))]
pub struct TuiState;

#[cfg(not(feature = "tui"))]
impl TuiState {
    pub fn new(_url: String, _concurrency: u32, _duration: std::time::Duration) -> Self {
        Self
    }
    pub fn report(&self) -> std::sync::Arc<tokio::sync::RwLock<crate::stats::Report>> {
        unimplemented!("TUI feature not enabled")
    }
    pub fn stop_flag(&self) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
        unimplemented!("TUI feature not enabled")
    }
    pub fn pause_flag(&self) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
        unimplemented!("TUI feature not enabled")
    }
}

#[cfg(not(feature = "tui"))]
pub fn run_tui(_state: TuiState) -> anyhow::Result<()> {
    unimplemented!("TUI feature not enabled")
}
