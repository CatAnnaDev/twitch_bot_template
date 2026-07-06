use std::time::Duration;

use crate::config::Config;
use crate::irc::Outbound;

struct Timer {
    every: Duration,
    message: &'static str,
}

const TIMERS: &[Timer] = &[
    Timer {
        every: Duration::from_secs(600),
        message: "Enjoying the stream? Drop a follow so you catch the next one.",
    },
    Timer {
        every: Duration::from_secs(900),
        message: "This channel runs a custom Rust bot. Type !commands to see what it does.",
    },
];

pub fn spawn(config: Config, out: Outbound) {
    for timer in TIMERS {
        let channels = config.channels.clone();
        let out = out.clone();
        let message = timer.message.to_string();
        let every = timer.every;

        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(every);
            ticker.tick().await;
            loop {
                ticker.tick().await;
                for channel in &channels {
                    out.say(channel, &message).await;
                }
            }
        });
    }
}
