#[derive(Default)]
pub struct DeploymentTargets {
    target_environments: Vec<DeploymentTarget>,
}
pub type DeploymentTarget = String;

impl DeploymentTargets {
    pub fn new(envs: Vec<String>) -> Self {
        Self { target_environments: envs }
    }

    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.target_environments.iter().map(|s| s.as_str())
    }
}
