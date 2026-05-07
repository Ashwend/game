use anyhow::Result;

use crate::{
    net::local::LocalGameSession,
    protocol::{ClientMessage, ServerMessage},
    save::{WorldSave, WorldStore},
    steam::AuthenticatedUser,
};

#[derive(Debug)]
pub enum ClientSession {
    Local(Box<LocalGameSession>),
}

impl ClientSession {
    pub fn start_singleplayer(save: WorldSave, user: &AuthenticatedUser) -> Result<Self> {
        LocalGameSession::start(save, user).map(|session| Self::Local(Box::new(session)))
    }

    pub fn send(&mut self, message: ClientMessage) -> Result<()> {
        match self {
            Self::Local(session) => {
                session.send(message);
                Ok(())
            }
        }
    }

    pub fn tick(&mut self, delta_seconds: f32) -> Result<Vec<ServerMessage>> {
        match self {
            Self::Local(session) => {
                session.tick(delta_seconds);
                Ok(session.drain())
            }
        }
    }

    pub fn shutdown(&mut self, store: &WorldStore) -> Result<()> {
        let _ = self.send(ClientMessage::Disconnect);
        match self {
            Self::Local(session) => session.persist(store)?,
        }
        Ok(())
    }
}
