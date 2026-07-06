use crate::db::Db;
use crate::helix::Helix;
use crate::irc::{IrcMessage, Outbound};

pub struct Builtin {
    pub name: &'static str,
    pub mod_only: bool,
    pub usage: &'static str,
    pub help: &'static str,
}

pub const BUILTINS: &[Builtin] = &[
    Builtin { name: "ping", mod_only: false, usage: "!ping", help: "replies pong" },
    Builtin { name: "count", mod_only: false, usage: "!count", help: "messages logged in this channel" },
    Builtin { name: "uptime", mod_only: false, usage: "!uptime", help: "current stream title / game / start" },
    Builtin { name: "commands", mod_only: false, usage: "!commands", help: "list available commands" },
    Builtin { name: "help", mod_only: false, usage: "!help <command>", help: "show usage for a command" },
    Builtin { name: "addcom", mod_only: true, usage: "!addcom <name> <response>", help: "create or replace a custom command" },
    Builtin { name: "delcom", mod_only: true, usage: "!delcom <name>", help: "delete a custom command" },
];

pub struct CommandContext<'a> {
    pub name: &'a str,
    pub args: Vec<&'a str>,
    pub channel: &'a str,
    pub message: &'a IrcMessage,
    pub db: &'a Db,
    pub helix: &'a Helix,
    pub out: &'a Outbound,
}

pub async fn dispatch(ctx: CommandContext<'_>) {
    if let Some(meta) = BUILTINS.iter().find(|b| b.name == ctx.name) {
        if meta.mod_only && !ctx.message.is_moderator() {
            return;
        }
    }

    match ctx.name {
        "ping" => builtin_ping(&ctx).await,
        "count" => builtin_count(&ctx).await,
        "uptime" => builtin_uptime(&ctx).await,
        "commands" => builtin_commands(&ctx).await,
        "help" => builtin_help(&ctx).await,
        "addcom" | "editcom" => builtin_addcom(&ctx).await,
        "delcom" => builtin_delcom(&ctx).await,
        other => builtin_custom(&ctx, other).await,
    }
}

async fn builtin_ping(ctx: &CommandContext<'_>) {
    let who = ctx.message.display_name().unwrap_or("friend");
    ctx.out
        .say(ctx.channel, &format!("pong, {who}"))
        .await;
}

async fn builtin_count(ctx: &CommandContext<'_>) {
    match ctx.db.message_count(ctx.channel).await {
        Ok(count) => {
            ctx.out
                .say(
                    ctx.channel,
                    &format!("{count} messages logged in this channel"),
                )
                .await
        }
        Err(err) => tracing::warn!(%err, "count query failed"),
    }
}

async fn builtin_uptime(ctx: &CommandContext<'_>) {
    match ctx.helix.stream_by_login(ctx.channel).await {
        Ok(Some(stream)) => {
            ctx.out
                .say(
                    ctx.channel,
                    &format!(
                        "live with \"{}\" ({}) since {}",
                        stream.title, stream.game_name, stream.started_at
                    ),
                )
                .await
        }
        Ok(None) => ctx.out.say(ctx.channel, "stream is offline").await,
        Err(err) => tracing::warn!(%err, "uptime query failed"),
    }
}

async fn builtin_commands(ctx: &CommandContext<'_>) {
    let builtins = BUILTINS
        .iter()
        .map(|b| b.name)
        .collect::<Vec<_>>()
        .join(", ");

    let custom = match ctx.db.all_commands().await {
        Ok(list) if !list.is_empty() => {
            let names = list
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            format!(" | custom: {names}")
        }
        _ => String::new(),
    };

    ctx.out
        .say(ctx.channel, &format!("builtins: {builtins}{custom}"))
        .await;
}

async fn builtin_help(ctx: &CommandContext<'_>) {
    let Some(arg) = ctx.args.first() else {
        ctx.out.say(ctx.channel, "usage: !help <command>").await;
        return;
    };
    let name = arg.trim_start_matches('!').to_ascii_lowercase();

    if let Some(builtin) = BUILTINS.iter().find(|b| b.name == name) {
        let scope = if builtin.mod_only { " (mods only)" } else { "" };
        ctx.out
            .say(
                ctx.channel,
                &format!("{} — {}{scope}", builtin.usage, builtin.help),
            )
            .await;
        return;
    }

    match ctx.db.custom_command(&name).await {
        Ok(Some(_)) => {
            ctx.out
                .say(ctx.channel, &format!("!{name} is a custom command"))
                .await
        }
        _ => ctx.out.say(ctx.channel, &format!("no command !{name}")).await,
    }
}

async fn builtin_addcom(ctx: &CommandContext<'_>) {
    let Some((name, response)) = ctx.args.split_first() else {
        ctx.out
            .say(ctx.channel, "usage: addcom <name> <response>")
            .await;
        return;
    };
    let name = name.trim_start_matches('!');
    if response.is_empty() {
        ctx.out
            .say(ctx.channel, "usage: addcom <name> <response>")
            .await;
        return;
    }
    let response = response.join(" ");
    match ctx.db.upsert_command(name, &response).await {
        Ok(()) => ctx.out.say(ctx.channel, &format!("saved !{name}")).await,
        Err(err) => tracing::warn!(%err, "upsert command failed"),
    }
}

async fn builtin_delcom(ctx: &CommandContext<'_>) {
    let Some(name) = ctx.args.first() else {
        ctx.out.say(ctx.channel, "usage: delcom <name>").await;
        return;
    };
    let name = name.trim_start_matches('!');
    match ctx.db.delete_command(name).await {
        Ok(true) => ctx.out.say(ctx.channel, &format!("deleted !{name}")).await,
        Ok(false) => ctx.out.say(ctx.channel, &format!("no command !{name}")).await,
        Err(err) => tracing::warn!(%err, "delete command failed"),
    }
}

async fn builtin_custom(ctx: &CommandContext<'_>, name: &str) {
    match ctx.db.custom_command(name).await {
        Ok(Some(cmd)) => {
            let who = ctx.message.display_name().unwrap_or("friend");
            let response = cmd.response.replace("$user", who);
            ctx.out.say(ctx.channel, &response).await;
            if let Err(err) = ctx.db.bump_command_use(name).await {
                tracing::warn!(%err, "bump command use failed");
            }
        }
        Ok(None) => {}
        Err(err) => tracing::warn!(%err, "custom command lookup failed"),
    }
}
