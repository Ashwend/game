use bevy::prelude::*;

#[derive(Resource, Debug, Clone, Copy)]
pub(crate) struct LookState {
    pub(crate) yaw: f32,
    pub(crate) pitch: f32,
    pub(crate) sensitivity: Vec2,
    /// Dev-only agent-drive override (the control socket's `walk` command):
    /// while set and unexpired, the movement input system feeds a forward
    /// input at the current look yaw, so a headless agent can actually walk
    /// the world and exercise collision/step-up end to end. Read-only in the
    /// input path (expiry is by deadline, not decrement); always `None`
    /// outside the harness.
    pub(crate) agent_walk: Option<AgentWalk>,
}

/// One agent-driven walk order: hold forward until `deadline`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct AgentWalk {
    pub(crate) deadline: std::time::Instant,
    pub(crate) run: bool,
}

impl LookState {
    /// The live walk order, if any hasn't expired yet.
    pub(crate) fn active_agent_walk(&self) -> Option<AgentWalk> {
        self.agent_walk
            .filter(|walk| std::time::Instant::now() < walk.deadline)
    }
}

impl Default for LookState {
    fn default() -> Self {
        Self {
            yaw: 0.0,
            pitch: -0.04,
            sensitivity: Vec2::new(0.0024, 0.0020),
            agent_walk: None,
        }
    }
}
