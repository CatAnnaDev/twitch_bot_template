use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use tokio::runtime::Handle;

use crate::commands;
use crate::config::Config;
use crate::db::{CustomCommand, Db, MessageRow};
use crate::feed::Feed;
use crate::irc::Outbound;

pub struct GuiContext {
    pub config: Config,
    pub db: Db,
    pub handle: Handle,
    pub out: Outbound,
    pub feed: Feed,
}

pub fn launch(ctx: GuiContext) {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([980.0, 660.0])
            .with_min_inner_size([720.0, 480.0])
            .with_title("twitch bot"),
        ..Default::default()
    };

    let _ = eframe::run_native(
        "twitch bot",
        native_options,
        Box::new(|cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            Ok(Box::new(Dashboard::new(ctx)))
        }),
    );
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Tab {
    Status,
    Commands,
    Chat,
    Events,
}

struct Dashboard {
    config: Config,
    db: Db,
    handle: Handle,
    out: Outbound,
    feed: Feed,

    tab: Tab,
    last_poll: Instant,

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
        let say_channel = ctx.config.channels.first().cloned().unwrap_or_default();
        let mut dashboard = Self {
            config: ctx.config,
            db: ctx.db,
            handle: ctx.handle,
            out: ctx.out,
            feed: ctx.feed,
            tab: Tab::Status,
            last_poll: Instant::now(),
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
        };
        dashboard.refresh();
        dashboard
    }

    fn refresh(&mut self) {
        let channels = self.config.channels.clone();
        let mut total = 0;
        self.counts.clear();
        for channel in channels {
            let count = self
                .handle
                .block_on(self.db.message_count(&channel))
                .unwrap_or(0);
            total += count;
            self.counts.insert(channel, count);
        }
        self.total_messages = total;
        self.commands = self.handle.block_on(self.db.all_commands()).unwrap_or_default();
        self.recent = self
            .handle
            .block_on(self.db.recent_messages(50))
            .unwrap_or_default();
        self.last_poll = Instant::now();
    }

    fn save_command(&mut self) {
        let name = self
            .editor_name
            .trim()
            .trim_start_matches('!')
            .to_ascii_lowercase();
        let response = self.editor_response.trim().to_string();

        if name.is_empty() || response.is_empty() {
            self.editor_status = "name and response are required".into();
            return;
        }
        if commands::BUILTINS.iter().any(|b| b.name == name) {
            self.editor_status = format!("!{name} is a builtin, choose another name");
            return;
        }

        match self.handle.block_on(self.db.upsert_command(&name, &response)) {
            Ok(()) => {
                self.editor_status = format!("saved !{name}");
                self.editor_name.clear();
                self.editor_response.clear();
                self.refresh();
            }
            Err(err) => self.editor_status = format!("error: {err}"),
        }
    }

    fn delete_command(&mut self, name: &str) {
        match self.handle.block_on(self.db.delete_command(name)) {
            Ok(_) => {
                self.editor_status = format!("deleted !{name}");
                self.refresh();
            }
            Err(err) => self.editor_status = format!("error: {err}"),
        }
    }

    fn send_message(&mut self) {
        let channel = self.say_channel.trim().trim_start_matches('#').to_string();
        let text = self.say_text.trim().to_string();
        if channel.is_empty() || text.is_empty() {
            self.say_status = "channel and message are required".into();
            return;
        }
        self.handle.block_on(self.out.say(&channel, &text));
        self.say_status = format!("sent to #{channel}");
        self.say_text.clear();
    }

    fn status_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Status");
        ui.add_space(8.0);
        egui::Grid::new("status_grid")
            .num_columns(2)
            .spacing([24.0, 8.0])
            .show(ui, |ui| {
                ui.label("bot");
                ui.strong(&self.config.bot_username);
                ui.end_row();
                ui.label("prefix");
                ui.strong(self.config.command_prefix.to_string());
                ui.end_row();
                ui.label("database");
                ui.strong(&self.config.database_url);
                ui.end_row();
                ui.label("messages logged");
                ui.strong(self.total_messages.to_string());
                ui.end_row();
            });

        ui.add_space(16.0);
        ui.strong("channels");
        ui.add_space(4.0);
        for channel in &self.config.channels {
            let count = self.counts.get(channel).copied().unwrap_or(0);
            ui.label(format!("#{channel}  —  {count} messages"));
        }
    }

    fn commands_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Custom commands");
        ui.add_space(8.0);

        egui::Grid::new("editor_grid")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label("name");
                ui.add(
                    egui::TextEdit::singleline(&mut self.editor_name)
                        .hint_text("hello")
                        .desired_width(220.0),
                );
                ui.end_row();
                ui.label("response");
                ui.add(
                    egui::TextEdit::multiline(&mut self.editor_response)
                        .hint_text("Hey $user, welcome!")
                        .desired_rows(2)
                        .desired_width(360.0),
                );
                ui.end_row();
            });

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("Save command").clicked() {
                self.save_command();
            }
            if ui.button("Clear").clicked() {
                self.editor_name.clear();
                self.editor_response.clear();
                self.editor_status.clear();
            }
        });
        if !self.editor_status.is_empty() {
            ui.add_space(4.0);
            ui.colored_label(egui::Color32::LIGHT_GREEN, &self.editor_status);
        }

        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);

        if self.commands.is_empty() {
            ui.label("no custom commands yet");
            return;
        }

        let mut to_edit: Option<(String, String)> = None;
        let mut to_delete: Option<String> = None;

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("commands_grid")
                    .num_columns(4)
                    .striped(true)
                    .spacing([16.0, 6.0])
                    .show(ui, |ui| {
                        ui.strong("command");
                        ui.strong("response");
                        ui.strong("uses");
                        ui.strong("");
                        ui.end_row();

                        for command in &self.commands {
                            ui.label(format!("!{}", command.name));
                            ui.label(&command.response);
                            ui.label(command.uses.to_string());
                            ui.horizontal(|ui| {
                                if ui.small_button("edit").clicked() {
                                    to_edit =
                                        Some((command.name.clone(), command.response.clone()));
                                }
                                if ui.small_button("delete").clicked() {
                                    to_delete = Some(command.name.clone());
                                }
                            });
                            ui.end_row();
                        }
                    });
            });

        if let Some((name, response)) = to_edit {
            self.editor_name = name;
            self.editor_response = response;
            self.editor_status.clear();
        }
        if let Some(name) = to_delete {
            self.delete_command(&name);
        }
    }

    fn chat_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Send message");
        ui.add_space(8.0);

        let channels = self.config.channels.clone();
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("say_channel")
                .selected_text(if self.say_channel.is_empty() {
                    "channel".to_string()
                } else {
                    format!("#{}", self.say_channel)
                })
                .show_ui(ui, |ui| {
                    for channel in &channels {
                        ui.selectable_value(
                            &mut self.say_channel,
                            channel.clone(),
                            format!("#{channel}"),
                        );
                    }
                });

            let response = ui.add(
                egui::TextEdit::singleline(&mut self.say_text)
                    .hint_text("message")
                    .desired_width(420.0),
            );
            let submit =
                response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

            if ui.button("Send").clicked() || submit {
                self.send_message();
            }
        });
        if !self.say_status.is_empty() {
            ui.add_space(4.0);
            ui.colored_label(egui::Color32::LIGHT_BLUE, &self.say_status);
        }

        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);
        ui.strong("recent chat");
        ui.add_space(4.0);

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for message in &self.recent {
                    ui.label(format!(
                        "#{}  {}: {}",
                        message.channel, message.login, message.text
                    ));
                }
            });
    }

    fn events_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Events");
        ui.add_space(8.0);

        let events = self.feed.snapshot();
        if events.is_empty() {
            ui.label("no events yet (follows, raids, stream online/offline show up here)");
            return;
        }

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for line in events.iter().rev() {
                    ui.label(line);
                }
            });
    }
}

impl eframe::App for Dashboard {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.last_poll.elapsed() >= Duration::from_secs(2) {
            self.refresh();
        }

        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.strong("twitch bot");
                ui.separator();
                ui.selectable_value(&mut self.tab, Tab::Status, "Status");
                ui.selectable_value(&mut self.tab, Tab::Commands, "Commands");
                ui.selectable_value(&mut self.tab, Tab::Chat, "Chat");
                ui.selectable_value(&mut self.tab, Tab::Events, "Events");
            });
            ui.add_space(4.0);
        });

        egui::CentralPanel::default().show(ctx, |ui| match self.tab {
            Tab::Status => self.status_tab(ui),
            Tab::Commands => self.commands_tab(ui),
            Tab::Chat => self.chat_tab(ui),
            Tab::Events => self.events_tab(ui),
        });

        ctx.request_repaint_after(Duration::from_secs(1));
    }
}
