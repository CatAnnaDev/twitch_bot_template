use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Tabs};
use ratatui::{Frame, Terminal};

const ACCENT: Color = Color::Cyan;
const STATUS_COLOR: Color = Color::Cyan;
const COMMANDS_COLOR: Color = Color::Green;
const CHAT_COLOR: Color = Color::LightBlue;
const EVENTS_COLOR: Color = Color::Magenta;
const LABEL: Color = Color::Gray;
const VALUE: Color = Color::White;

use crate::commands;
use crate::config::Config;
use crate::db::{CustomCommand, Db, MessageRow};
use crate::error::BotResult;
use crate::feed::Feed;
use crate::irc::Outbound;

pub struct TuiContext {
    pub config: Config,
    pub db: Db,
    pub out: Outbound,
    pub feed: Feed,
}

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Status,
    Commands,
    Chat,
    Events,
}

#[derive(PartialEq)]
enum Mode {
    Normal,
    Insert,
}

enum InputTarget {
    Command,
    Chat,
}

struct Tui {
    config: Config,
    db: Db,
    out: Outbound,
    feed: Feed,
    channels: Vec<String>,

    tab: Tab,
    mode: Mode,
    input: String,
    input_target: InputTarget,

    counts: BTreeMap<String, i64>,
    total: i64,
    commands: Vec<CustomCommand>,
    recent: Vec<MessageRow>,

    cmd_selected: usize,
    say_channel_idx: usize,
    status: String,
}

pub async fn run(ctx: TuiContext) -> BotResult<()> {
    let mut app = Tui::new(ctx);
    app.refresh().await;

    let mut terminal = ratatui::init();
    let result = app.event_loop(&mut terminal).await;
    ratatui::restore();
    result
}

impl Tui {
    fn new(ctx: TuiContext) -> Self {
        let channels = ctx.config.channels.clone();
        Self {
            config: ctx.config,
            db: ctx.db,
            out: ctx.out,
            feed: ctx.feed,
            channels,
            tab: Tab::Status,
            mode: Mode::Normal,
            input: String::new(),
            input_target: InputTarget::Command,
            counts: BTreeMap::new(),
            total: 0,
            commands: Vec::new(),
            recent: Vec::new(),
            cmd_selected: 0,
            say_channel_idx: 0,
            status: "ready".into(),
        }
    }

    async fn event_loop<B: ratatui::backend::Backend>(
        &mut self,
        terminal: &mut Terminal<B>,
    ) -> BotResult<()> {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Event>(64);
        let stop = Arc::new(AtomicBool::new(false));
        spawn_input_reader(tx, stop.clone());

        let mut refresh = tokio::time::interval(Duration::from_secs(1));

        loop {
            terminal.draw(|frame| self.draw(frame))?;

            tokio::select! {
                _ = refresh.tick() => self.refresh().await,
                event = rx.recv() => {
                    let Some(Event::Key(key)) = event else { continue };
                    if self.handle_key(key).await {
                        break;
                    }
                }
            }
        }

        stop.store(true, Ordering::Relaxed);
        Ok(())
    }

    async fn refresh(&mut self) {
        let channels = self.channels.clone();
        let mut total = 0;
        self.counts.clear();
        for channel in channels {
            let count = self.db.message_count(&channel).await.unwrap_or(0);
            total += count;
            self.counts.insert(channel, count);
        }
        self.total = total;
        self.commands = self.db.all_commands().await.unwrap_or_default();
        self.recent = self.db.recent_messages(50).await.unwrap_or_default();

        if !self.commands.is_empty() && self.cmd_selected >= self.commands.len() {
            self.cmd_selected = self.commands.len() - 1;
        }
    }

    async fn handle_key(&mut self, key: KeyEvent) -> bool {
        if key.kind != KeyEventKind::Press {
            return false;
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return true;
        }
        match self.mode {
            Mode::Normal => return self.handle_normal(key).await,
            Mode::Insert => self.handle_insert(key).await,
        }
        false
    }

    async fn handle_normal(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char('q') => return true,
            KeyCode::Tab => self.tab = next_tab(self.tab),
            KeyCode::BackTab => self.tab = prev_tab(self.tab),
            KeyCode::Char('1') => self.tab = Tab::Status,
            KeyCode::Char('2') => self.tab = Tab::Commands,
            KeyCode::Char('3') => self.tab = Tab::Chat,
            KeyCode::Char('4') => self.tab = Tab::Events,
            KeyCode::Down => self.move_selection(1),
            KeyCode::Up => self.move_selection(-1),
            KeyCode::Left if self.tab == Tab::Chat => self.cycle_channel(-1),
            KeyCode::Right if self.tab == Tab::Chat => self.cycle_channel(1),
            KeyCode::Char('a') if self.tab == Tab::Commands => {
                self.mode = Mode::Insert;
                self.input_target = InputTarget::Command;
                self.input.clear();
                self.status = "add command: <name> <response>".into();
            }
            KeyCode::Char('d') if self.tab == Tab::Commands => self.delete_selected().await,
            KeyCode::Char('i') if self.tab == Tab::Chat => {
                self.mode = Mode::Insert;
                self.input_target = InputTarget::Chat;
                self.input.clear();
                self.status = "type a message, Enter to send".into();
            }
            _ => {}
        }
        false
    }

    async fn handle_insert(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.input.clear();
                self.status = "cancelled".into();
            }
            KeyCode::Enter => {
                self.submit().await;
                self.mode = Mode::Normal;
            }
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Char(c) => self.input.push(c),
            _ => {}
        }
    }

    async fn submit(&mut self) {
        match self.input_target {
            InputTarget::Command => {
                let mut parts = self.input.trim().splitn(2, ' ');
                let name = parts
                    .next()
                    .unwrap_or("")
                    .trim_start_matches('!')
                    .to_ascii_lowercase();
                let response = parts.next().unwrap_or("").trim().to_string();

                if name.is_empty() || response.is_empty() {
                    self.status = "need: <name> <response>".into();
                } else if commands::BUILTINS.iter().any(|b| b.name == name) {
                    self.status = format!("!{name} is a builtin, choose another name");
                } else {
                    match self.db.upsert_command(&name, &response).await {
                        Ok(()) => {
                            self.status = format!("saved !{name}");
                            self.refresh().await;
                        }
                        Err(err) => self.status = format!("error: {err}"),
                    }
                }
            }
            InputTarget::Chat => {
                let text = self.input.trim().to_string();
                if let Some(channel) = self.channels.get(self.say_channel_idx).cloned() {
                    if !text.is_empty() {
                        self.out.say(&channel, &text).await;
                        self.status = format!("sent to #{channel}");
                    }
                }
            }
        }
        self.input.clear();
    }

    async fn delete_selected(&mut self) {
        if let Some(command) = self.commands.get(self.cmd_selected) {
            let name = command.name.clone();
            match self.db.delete_command(&name).await {
                Ok(_) => {
                    self.status = format!("deleted !{name}");
                    self.refresh().await;
                }
                Err(err) => self.status = format!("error: {err}"),
            }
        }
    }

    fn move_selection(&mut self, delta: i32) {
        if self.tab != Tab::Commands || self.commands.is_empty() {
            return;
        }
        let last = self.commands.len() - 1;
        let next = self.cmd_selected as i32 + delta;
        self.cmd_selected = next.clamp(0, last as i32) as usize;
    }

    fn cycle_channel(&mut self, delta: i32) {
        if self.channels.is_empty() {
            return;
        }
        let n = self.channels.len() as i32;
        let next = (self.say_channel_idx as i32 + delta).rem_euclid(n);
        self.say_channel_idx = next as usize;
    }

    fn draw(&self, frame: &mut Frame) {
        let chunks = Layout::vertical([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(frame.area());

        let titles = ["Status", "Commands", "Chat", "Events"].map(|t| {
            Span::styled(
                format!(" {t} "),
                Style::default().fg(Color::DarkGray),
            )
        });
        let tabs = Tabs::new(titles.to_vec())
            .select(self.tab as usize)
            .divider(Span::styled("|", Style::default().fg(Color::DarkGray)))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(ACCENT))
                    .title(Span::styled(
                        " twitch bot ",
                        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                    )),
            )
            .highlight_style(
                Style::default()
                    .fg(Color::Black)
                    .bg(self.tab_color())
                    .add_modifier(Modifier::BOLD),
            );
        frame.render_widget(tabs, chunks[0]);

        match self.tab {
            Tab::Status => self.draw_status(frame, chunks[1]),
            Tab::Commands => self.draw_commands(frame, chunks[1]),
            Tab::Chat => self.draw_chat(frame, chunks[1]),
            Tab::Events => self.draw_events(frame, chunks[1]),
        }

        self.draw_footer(frame, chunks[2]);
    }

    fn tab_color(&self) -> Color {
        match self.tab {
            Tab::Status => STATUS_COLOR,
            Tab::Commands => COMMANDS_COLOR,
            Tab::Chat => CHAT_COLOR,
            Tab::Events => EVENTS_COLOR,
        }
    }

    fn draw_status(&self, frame: &mut Frame, area: Rect) {
        let field = |label: &'static str, value: String| {
            Line::from(vec![
                Span::styled(format!("{label:<10}"), Style::default().fg(LABEL)),
                Span::styled(
                    value,
                    Style::default().fg(VALUE).add_modifier(Modifier::BOLD),
                ),
            ])
        };

        let mut lines = vec![
            field("bot", self.config.bot_username.clone()),
            field("prefix", self.config.command_prefix.to_string()),
            field("database", self.config.database_url.clone()),
            field("messages", self.total.to_string()),
            Line::from(""),
            Line::from(Span::styled(
                "channels",
                Style::default().fg(STATUS_COLOR).add_modifier(Modifier::BOLD),
            )),
        ];
        for channel in &self.channels {
            let count = self.counts.get(channel).copied().unwrap_or(0);
            lines.push(Line::from(vec![
                Span::styled(format!("  #{channel}"), Style::default().fg(CHAT_COLOR)),
                Span::styled(
                    format!("  -  {count} messages"),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
        }
        frame.render_widget(Paragraph::new(lines).block(section(" Status ", STATUS_COLOR)), area);
    }

    fn draw_commands(&self, frame: &mut Frame, area: Rect) {
        let items: Vec<ListItem> = if self.commands.is_empty() {
            vec![ListItem::new(Span::styled(
                "no custom commands yet - press [a] to add",
                Style::default().fg(Color::DarkGray),
            ))]
        } else {
            self.commands
                .iter()
                .map(|c| {
                    ListItem::new(Line::from(vec![
                        Span::styled(
                            format!("!{}", c.name),
                            Style::default()
                                .fg(COMMANDS_COLOR)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled("  ->  ", Style::default().fg(Color::DarkGray)),
                        Span::styled(c.response.clone(), Style::default().fg(VALUE)),
                        Span::styled(
                            format!("   ({} uses)", c.uses),
                            Style::default().fg(Color::Yellow),
                        ),
                    ]))
                })
                .collect()
        };

        let list = List::new(items)
            .block(section(
                " Commands   [a] add   [d] delete   [Up/Down] select ",
                COMMANDS_COLOR,
            ))
            .highlight_style(
                Style::default()
                    .bg(COMMANDS_COLOR)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("> ");

        let mut state = ListState::default();
        if !self.commands.is_empty() {
            state.select(Some(self.cmd_selected));
        }
        frame.render_stateful_widget(list, area, &mut state);
    }

    fn draw_chat(&self, frame: &mut Frame, area: Rect) {
        let channel = self
            .channels
            .get(self.say_channel_idx)
            .cloned()
            .unwrap_or_default();
        let items: Vec<ListItem> = self
            .recent
            .iter()
            .map(|m| {
                ListItem::new(Line::from(vec![
                    Span::styled(format!("#{} ", m.channel), Style::default().fg(CHAT_COLOR)),
                    Span::styled(
                        format!("{}: ", m.login),
                        Style::default()
                            .fg(Color::LightMagenta)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(m.text.clone(), Style::default().fg(VALUE)),
                ]))
            })
            .collect();
        let list = List::new(items).block(section(
            format!(" Chat   posting to #{channel}   [i] type   [Left/Right] channel "),
            CHAT_COLOR,
        ));
        frame.render_widget(list, area);
    }

    fn draw_events(&self, frame: &mut Frame, area: Rect) {
        let events = self.feed.snapshot();
        let items: Vec<ListItem> = if events.is_empty() {
            vec![ListItem::new(Span::styled(
                "no events yet (follows, raids, stream online/offline)",
                Style::default().fg(Color::DarkGray),
            ))]
        } else {
            events
                .iter()
                .rev()
                .map(|line| ListItem::new(Span::styled(line.clone(), event_style(line))))
                .collect()
        };
        frame.render_widget(List::new(items).block(section(" Events ", EVENTS_COLOR)), area);
    }

    fn draw_footer(&self, frame: &mut Frame, area: Rect) {
        if self.mode == Mode::Insert {
            let line = Line::from(vec![
                Span::styled("> ", Style::default().fg(Color::Yellow)),
                Span::styled(self.input.clone(), Style::default().fg(VALUE)),
                Span::styled("_", Style::default().fg(Color::Yellow)),
            ]);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow))
                .title(Span::styled(
                    " input  [Enter] submit  [Esc] cancel ",
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                ));
            frame.render_widget(Paragraph::new(line).block(block), area);
            return;
        }

        let keys = match self.tab {
            Tab::Commands => "[a] add  [d] delete  [Up/Down] select",
            Tab::Chat => "[i] type  [Left/Right] channel",
            _ => "[Tab] switch tabs",
        };
        let line = Line::from(vec![
            Span::styled("[Tab] ", Style::default().fg(ACCENT)),
            Span::styled("tabs  ", Style::default().fg(Color::DarkGray)),
            Span::styled("[q] ", Style::default().fg(ACCENT)),
            Span::styled("quit   ", Style::default().fg(Color::DarkGray)),
            Span::styled(keys, Style::default().fg(self.tab_color())),
            Span::styled("   ", Style::default()),
            Span::styled(
                self.status.clone(),
                Style::default().fg(Color::Yellow).add_modifier(Modifier::ITALIC),
            ),
        ]);
        frame.render_widget(
            Paragraph::new(line).block(section(" keys ", Color::DarkGray)),
            area,
        );
    }
}

fn section(title: impl Into<String>, color: Color) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(color))
        .title(Span::styled(
            title.into(),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ))
}

fn event_style(line: &str) -> Style {
    let color = if line.starts_with("[raid]") {
        Color::Magenta
    } else if line.starts_with("[follow]") {
        Color::Green
    } else if line.starts_with("[online]") {
        Color::LightGreen
    } else if line.starts_with("[offline]") {
        Color::Red
    } else {
        Color::Gray
    };
    Style::default().fg(color)
}

fn spawn_input_reader(tx: tokio::sync::mpsc::Sender<Event>, stop: Arc<AtomicBool>) {
    std::thread::spawn(move || loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        match ratatui::crossterm::event::poll(Duration::from_millis(150)) {
            Ok(true) => match ratatui::crossterm::event::read() {
                Ok(event) => {
                    if tx.blocking_send(event).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            },
            Ok(false) => {}
            Err(_) => break,
        }
    });
}

fn next_tab(tab: Tab) -> Tab {
    match tab {
        Tab::Status => Tab::Commands,
        Tab::Commands => Tab::Chat,
        Tab::Chat => Tab::Events,
        Tab::Events => Tab::Status,
    }
}

fn prev_tab(tab: Tab) -> Tab {
    match tab {
        Tab::Status => Tab::Events,
        Tab::Commands => Tab::Status,
        Tab::Chat => Tab::Commands,
        Tab::Events => Tab::Chat,
    }
}
