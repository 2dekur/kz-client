//! Discord Rich Presence sur un thread dédié (le client IPC n'est pas `Send`-friendly).
//! Reconnexion automatique si Discord est lancé après le launcher.

use std::{
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender},
    thread,
    time::Duration,
};

use discord_rich_presence::{activity, DiscordIpc, DiscordIpcClient};

#[derive(Debug, Clone)]
pub enum Presence {
    Idle,
    Playing {
        username: String,
        label: String,
        started: i64,
    },
}

pub struct Discord {
    tx: Option<Sender<Presence>>,
}

impl Discord {
    /// `app_id` absent → Rich Presence désactivé sans erreur.
    pub fn start(app_id: Option<String>) -> Self {
        let Some(id) = app_id else {
            return Self { tx: None };
        };
        let (tx, rx) = mpsc::channel();
        match thread::Builder::new().name("kz-discord".into()).spawn(move || run(id, rx)) {
            Ok(_) => Self { tx: Some(tx) },
            Err(e) => {
                eprintln!("Discord RPC désactivé : {e}");
                Self { tx: None }
            }
        }
    }

    pub fn set(&self, presence: Presence) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(presence);
        }
    }
}

fn run(id: String, rx: Receiver<Presence>) {
    let mut client: Option<DiscordIpcClient> = None;
    let mut current = Presence::Idle;
    let mut dirty = true;
    loop {
        if client.is_none() {
            client = connect(&id);
            if client.is_some() {
                dirty = true;
            }
        }
        if dirty {
            if let Some(c) = client.as_mut() {
                if apply(c, &current).is_ok() {
                    dirty = false;
                } else {
                    let _ = c.close();
                    client = None;
                }
            }
        }
        match rx.recv_timeout(Duration::from_secs(10)) {
            Ok(p) => {
                current = p;
                dirty = true;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                if let Some(mut c) = client {
                    let _ = c.close();
                }
                return;
            }
        }
    }
}

fn connect(id: &str) -> Option<DiscordIpcClient> {
    let mut c = DiscordIpcClient::new(id).ok()?;
    c.connect().ok()?;
    Some(c)
}

fn apply(c: &mut DiscordIpcClient, presence: &Presence) -> Result<(), Box<dyn std::error::Error>> {
    let assets = activity::Assets::new().large_image("kz_logo").large_text("KZ Client");
    match presence {
        Presence::Idle => {
            c.set_activity(activity::Activity::new().details("Dans le launcher").state("Prêt à jouer").assets(assets))
        }
        Presence::Playing { username, label, started } => {
            let state = format!("Pseudo : {username}");
            let activity = activity::Activity::new()
                .details(label)
                .state(&state)
                .timestamps(activity::Timestamps::new().start(*started))
                .assets(assets);
            c.set_activity(activity)
        }
    }
}
