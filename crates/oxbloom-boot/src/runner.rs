use async_trait::async_trait;

use crate::arguments::ApplicationArguments;
use crate::error::RunnerError;

/// Code that runs once after the container is built, before services start.
///
/// Register it as a bean that `provides` `dyn ApplicationRunner`. Boot runs every
/// application runner in DI order (`order`, then registration order), before any
/// [`CommandLineRunner`]. An error stops the run with a `StartupError` in the
/// `Runner` phase. A panic is not caught. Implement it with
/// [`async_trait`](crate::async_trait).
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_boot::async_trait;
/// use oxbloom_boot::{ApplicationArguments, ApplicationRunner, OxbloomApplication, RunnerError};
/// use oxbloom_config::Config;
///
/// struct Migrate;
///
/// #[async_trait]
/// impl ApplicationRunner for Migrate {
///     async fn run(&self, args: &ApplicationArguments) -> Result<(), RunnerError> {
///         if args.option("dry-run").is_some() {
///             return Err(RunnerError::new("dry run requested"));
///         }
///         Ok(())
///     }
/// }
///
/// # tokio::runtime::Builder::new_current_thread().build()?.block_on(async {
/// let status = OxbloomApplication::builder()
///     .with_config(Config::from_str("")?)
///     .output(std::io::sink())
///     .configure(|b| {
///         b.instance(Migrate)
///             .provides(|m: Arc<Migrate>| -> Arc<dyn ApplicationRunner> { m })
///     })
///     .run()
///     .await?;
/// assert_eq!(status.code(), 0);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # })?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[async_trait]
pub trait ApplicationRunner: Send + Sync + 'static {
    /// Runs with the parsed command line.
    ///
    /// ```
    /// use oxbloom_boot::async_trait;
    /// use oxbloom_boot::{ApplicationArguments, ApplicationRunner, RunnerError};
    ///
    /// struct Greet;
    ///
    /// #[async_trait]
    /// impl ApplicationRunner for Greet {
    ///     async fn run(&self, args: &ApplicationArguments) -> Result<(), RunnerError> {
    ///         println!("hello {}", args.option("name").unwrap_or("world"));
    ///         Ok(())
    ///     }
    /// }
    ///
    /// # tokio::runtime::Builder::new_current_thread().build()?.block_on(async {
    /// Greet.run(&ApplicationArguments::new(["--name=Oxbloom"])).await?;
    /// # Ok::<(), RunnerError>(())
    /// # })?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    async fn run(&self, args: &ApplicationArguments) -> Result<(), RunnerError>;
}

/// Code that runs once with the raw command-line arguments, after every
/// [`ApplicationRunner`].
///
/// Register it as a bean that `provides` `dyn CommandLineRunner`. Boot runs every
/// command-line runner in DI order (`order`, then registration order). An error
/// stops the run with a `StartupError` in the `Runner` phase. A panic is not
/// caught. Implement it with [`async_trait`](crate::async_trait).
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_boot::async_trait;
/// use oxbloom_boot::{CommandLineRunner, OxbloomApplication, RunnerError, StartupPhase};
/// use oxbloom_config::Config;
///
/// struct RequireCommand;
///
/// #[async_trait]
/// impl CommandLineRunner for RequireCommand {
///     async fn run(&self, args: &[String]) -> Result<(), RunnerError> {
///         if args.is_empty() {
///             return Err(RunnerError::new("no command given"));
///         }
///         Ok(())
///     }
/// }
///
/// # tokio::runtime::Builder::new_current_thread().build()?.block_on(async {
/// let err = OxbloomApplication::builder()
///     .with_config(Config::from_str("")?)
///     .output(std::io::sink())
///     .configure(|b| {
///         b.instance(RequireCommand)
///             .provides(|r: Arc<RequireCommand>| -> Arc<dyn CommandLineRunner> { r })
///     })
///     .run()
///     .await
///     .unwrap_err();
/// assert_eq!(err.phase(), StartupPhase::Runner);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # })?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[async_trait]
pub trait CommandLineRunner: Send + Sync + 'static {
    /// Runs with the arguments as given, program name excluded.
    ///
    /// ```
    /// use oxbloom_boot::async_trait;
    /// use oxbloom_boot::{CommandLineRunner, RunnerError};
    ///
    /// struct Count;
    ///
    /// #[async_trait]
    /// impl CommandLineRunner for Count {
    ///     async fn run(&self, args: &[String]) -> Result<(), RunnerError> {
    ///         println!("{} arguments", args.len());
    ///         Ok(())
    ///     }
    /// }
    ///
    /// # tokio::runtime::Builder::new_current_thread().build()?.block_on(async {
    /// Count.run(&["serve".to_owned()]).await?;
    /// # Ok::<(), RunnerError>(())
    /// # })?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    async fn run(&self, args: &[String]) -> Result<(), RunnerError>;
}
