use std::collections::BTreeMap;
use std::time::Duration;

use iced::widget::{button, column, container, pick_list, row, scrollable, text, text_input};
use iced::{time, Element, Length, Subscription, Task, Theme};

use crate::commands;
use crate::config::Config;
use crate::db::{CustomCommand, Db, MessageRow};
use crate::feed::Feed;
use crate::irc::Outbound;

pub struct GuiContext {
    pub config: Config,
    pub db: Db,
    pub out: Outbound,
    pub feed: Feed,
}

pub fn launch(ctx: GuiContext) {
    let _ = iced::application("twitch bot", Dashboard::update, Dashboard::view)
        .subscription(Dashboard::subscription)
        .theme(|_| Theme::Dark)
        .window_size(iced::Size::new(980.0, 660.0))
        .run_with(move || {
            let dashboard = Dashboard::new(ctx);
            let task = dashboard.load();
            (dashboard, task)
        });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Status,
    Commands,
    Chat,
    Events,
}

#[derive(Debug, Clone, Default)]
struct Snapshot {
    counts: BTreeMap<String, i64>,
    total: i64,
    commands: Vec<CustomCommand>,
    recent: Vec<MessageRow>,
}

#[derive(Debug, Clone)]
enum Message {
    TabSelected(Tab),
    Tick,
    Refreshed(Snapshot),
    EditorNameChanged(String),
    EditorResponseChanged(String),
    SaveCommand,
    CommandSaved(Result<String, String>),
    ClearEditor,
    EditCommand(String, String),
    DeleteCommand(String),
    CommandDeleted(Result<String, String>),
    SayChannelSelected(String),
    SayTextChanged(String),
    SendMessage,
    MessageSent(String),
}

struct Dashboard {
    config: Config,
    db: Db,
    out: Outbound,
    feed: Feed,
    channels: Vec<String>,

    tab: Tab,
    counts: BTreeMap<String, i64>,
    total_messages: i64,
    commands: Vec<CustomCommand>,
    recent: Vec<MessageRow>,

    editor_name: String,
    editor_response: String,
    editor_status: String,

    say_channel: String,
    say_text: String,
    say_status: String,
}

impl Dashboard {
    fn new(ctx: GuiContext) -> Self {
        let channels = ctx.config.channels.clone();
        let say_channel = channels.first().cloned().unwrap_or_default();
        Self {
            config: ctx.config,
            db: ctx.db,
            out: ctx.out,
            feed: ctx.feed,
            channels,
            tab: Tab::Status,
            counts: BTreeMap::new(),
            total_messages: 0,
            commands: Vec::new(),
            recent: Vec::new(),
            editor_name: String::new(),
            editor_response: String::new(),
            editor_status: String::new(),
            say_channel,
            say_text: String::new(),
            say_status: String::new(),
        }
    }

    fn load(&self) -> Task<Message> {
        let db = self.db.clone();
        let channels = self.channels.clone();
        Task::perform(
            async move {
                let mut counts = BTreeMap::new();
                let mut total = 0;
                for channel in &channels {
                    let count = db.message_count(channel).await.unwrap_or(0);
                    total += count;
                    counts.insert(channel.clone(), count);
                }
                Snapshot {
                    counts,
                    total,
                    commands: db.all_commands().await.unwrap_or_default(),
                    recent: db.recent_messages(50).await.unwrap_or_default(),
                }
            },
            Message::Refreshed,
        )
    }

    fn subscription(&self) -> Subscription<Message> {
        time::every(Duration::from_secs(2)).map(|_| Message::Tick)
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::TabSelected(tab) => {
                self.tab = tab;
                Task::none()
            }
            Message::Tick => self.load(),
            Message::Refreshed(snapshot) => {
                self.counts = snapshot.counts;
                self.total_messages = snapshot.total;
                self.commands = snapshot.commands;
                self.recent = snapshot.recent;
                Task::none()
            }
            Message::EditorNameChanged(value) => {
                self.editor_name = value;
                Task::none()
            }
            Message::EditorResponseChanged(value) => {
                self.editor_response = value;
                Task::none()
            }
            Message::ClearEditor => {
                self.editor_name.clear();
                self.editor_response.clear();
                self.editor_status.clear();
                Task::none()
            }
            Message::EditCommand(name, response) => {
                self.editor_name = name;
                self.editor_response = response;
                self.editor_status.clear();
                Task::none()
            }
            Message::SaveCommand => self.save_command(),
            Message::CommandSaved(Ok(name)) => {
                self.editor_status = format!("saved !{name}");
                self.editor_name.clear();
                self.editor_response.clear();
                self.load()
            }
            Message::CommandSaved(Err(err)) => {
                self.editor_status = err;
                Task::none()
            }
            Message::DeleteCommand(name) => {
                let db = self.db.clone();
                Task::perform(
                    async move {
                        db.delete_command(&name)
                            .await
                            .map(|_| name)
                            .map_err(|e| e.to_string())
                    },
                    Message::CommandDeleted,
                )
            }
            Message::CommandDeleted(Ok(name)) => {
                self.editor_status = format!("deleted !{name}");
                self.load()
            }
            Message::CommandDeleted(Err(err)) => {
                self.editor_status = err;
                Task::none()
            }
            Message::SayChannelSelected(channel) => {
                self.say_channel = channel;
                Task::none()
            }
            Message::SayTextChanged(value) => {
                self.say_text = value;
                Task::none()
            }
            Message::SendMessage => self.send_message(),
            Message::MessageSent(channel) => {
                self.say_status = format!("sent to #{channel}");
                self.say_text.clear();
                Task::none()
            }
        }
    }

    fn save_command(&mut self) -> Task<Message> {
        let name = self
            .editor_name
            .trim()
            .trim_start_matches('!')
            .to_ascii_lowercase();
        let response = self.editor_response.trim().to_string();

        if name.is_empty() || response.is_empty() {
            self.editor_status = "name and response are required".into();
            return Task::none();
        }
        if commands::BUILTINS.iter().any(|b| b.name == name) {
            self.editor_status = format!("!{name} is a builtin, choose another name");
            return Task::none();
        }

        let db = self.db.clone();
        Task::perform(
            async move {
                db.upsert_command(&name, &response)
                    .await
                    .map(|_| name)
                    .map_err(|e| e.to_string())
            },
            Message::CommandSaved,
        )
    }

    fn send_message(&mut self) -> Task<Message> {
        let channel = self.say_channel.trim().trim_start_matches('#').to_string();
        let text = self.say_text.trim().to_string();
        if channel.is_empty() || text.is_empty() {
            self.say_status = "channel and message are required".into();
            return Task::none();
        }

        let out = self.out.clone();
        Task::perform(
            async move {
                out.say(&channel, &text).await;
                channel
            },
            Message::MessageSent,
        )
    }

    fn view(&self) -> Element<'_, Message> {
        let tabs = row![
            self.tab_button("Status", Tab::Status),
            self.tab_button("Commands", Tab::Commands),
            self.tab_button("Chat", Tab::Chat),
            self.tab_button("Events", Tab::Events),
        ]
        .spacing(8);

        let header = row![
            text("twitch bot").size(20),
            container(tabs)
                .width(Length::Fill)
                .align_x(iced::alignment::Horizontal::Right),
        ]
        .spacing(16)
        .align_y(iced::alignment::Vertical::Center);

        let body = match self.tab {
            Tab::Status => self.status_tab(),
            Tab::Commands => self.commands_tab(),
            Tab::Chat => self.chat_tab(),
            Tab::Events => self.events_tab(),
        };

        container(column![header, body].spacing(16))
            .padding(20)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn tab_button<'a>(&self, label: &'a str, tab: Tab) -> Element<'a, Message> {
        let style = if self.tab == tab {
            button::primary
        } else {
            button::secondary
        };
        button(text(label))
            .style(style)
            .on_press(Message::TabSelected(tab))
            .into()
    }

    fn status_tab(&self) -> Element<'_, Message> {
        let mut info = column![
            info_row("bot", &self.config.bot_username),
            info_row("prefix", &self.config.command_prefix.to_string()),
            info_row("database", &self.config.database_url),
            info_row("messages logged", &self.total_messages.to_string()),
        ]
        .spacing(6);

        info = info.push(text("channels").size(16));
        for channel in &self.channels {
            let count = self.counts.get(channel).copied().unwrap_or(0);
            info = info.push(text(format!("#{channel}  -  {count} messages")));
        }

        column![text("Status").size(22), info].spacing(12).into()
    }

    fn commands_tab(&self) -> Element<'_, Message> {
        let editor = column![
            row![
                text("name").width(90),
                text_input("hello", &self.editor_name)
                    .on_input(Message::EditorNameChanged)
                    .width(240),
            ]
            .spacing(8)
            .align_y(iced::alignment::Vertical::Center),
            row![
                text("response").width(90),
                text_input("Hey $user, welcome!", &self.editor_response)
                    .on_input(Message::EditorResponseChanged)
                    .on_submit(Message::SaveCommand)
                    .width(420),
            ]
            .spacing(8)
            .align_y(iced::alignment::Vertical::Center),
            row![
                button(text("Save command")).on_press(Message::SaveCommand),
                button(text("Clear"))
                    .style(button::secondary)
                    .on_press(Message::ClearEditor),
            ]
            .spacing(8),
            text(&self.editor_status),
        ]
        .spacing(10);

        let mut list = column![row![
            text("command").width(140),
            text("response").width(Length::Fill),
            text("uses").width(60),
            text("").width(140),
        ]
        .spacing(12)]
        .spacing(6);

        if self.commands.is_empty() {
            list = list.push(text("no custom commands yet"));
        } else {
            for command in &self.commands {
                list = list.push(
                    row![
                        text(format!("!{}", command.name)).width(140),
                        text(command.response.clone()).width(Length::Fill),
                        text(command.uses.to_string()).width(60),
                        row![
                            button(text("edit").size(13))
                                .style(button::secondary)
                                .on_press(Message::EditCommand(
                                    command.name.clone(),
                                    command.response.clone(),
                                )),
                            button(text("delete").size(13))
                                .style(button::danger)
                                .on_press(Message::DeleteCommand(command.name.clone())),
                        ]
                        .spacing(6)
                        .width(140),
                    ]
                    .spacing(12)
                    .align_y(iced::alignment::Vertical::Center),
                );
            }
        }

        column![
            text("Custom commands").size(22),
            editor,
            scrollable(list).height(Length::Fill),
        ]
        .spacing(12)
        .into()
    }

    fn chat_tab(&self) -> Element<'_, Message> {
        let picker = pick_list(
            self.channels.clone(),
            Some(self.say_channel.clone()),
            Message::SayChannelSelected,
        );

        let composer = row![
            picker,
            text_input("message", &self.say_text)
                .on_input(Message::SayTextChanged)
                .on_submit(Message::SendMessage)
                .width(Length::Fill),
            button(text("Send")).on_press(Message::SendMessage),
        ]
        .spacing(8)
        .align_y(iced::alignment::Vertical::Center);

        let mut lines = column![].spacing(4);
        for message in &self.recent {
            lines = lines.push(text(format!(
                "#{}  {}: {}",
                message.channel, message.login, message.text
            )));
        }

        column![
            text("Send message").size(22),
            composer,
            text(&self.say_status),
            text("recent chat").size(16),
            scrollable(lines).height(Length::Fill),
        ]
        .spacing(12)
        .into()
    }

    fn events_tab(&self) -> Element<'_, Message> {
        let events = self.feed.snapshot();
        let mut lines = column![].spacing(4);

        if events.is_empty() {
            lines = lines.push(text(
                "no events yet (follows, raids, stream online/offline show up here)",
            ));
        } else {
            for line in events.iter().rev() {
                lines = lines.push(text(line.clone()));
            }
        }

        column![
            text("Events").size(22),
            scrollable(lines).height(Length::Fill)
        ]
        .spacing(12)
        .into()
    }
}

fn info_row<'a>(label: &'a str, value: &str) -> Element<'a, Message> {
    row![text(label).width(160), text(value.to_string())]
        .spacing(12)
        .into()
}
