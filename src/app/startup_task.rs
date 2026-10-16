use async_trait::async_trait;

#[async_trait]
pub trait StartupTask: Send + Sync {
    fn name(&self) -> &str;
    async fn run(&self);
}
