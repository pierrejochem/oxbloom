use std::process::ExitCode;

/// Contributes to the process exit code of a run.
///
/// Register a bean that `provides` `dyn ExitCodeGenerator`. After the services
/// have stopped and the container is shut down, boot asks every generator for
/// its code, maps each one to a process exit code and the run finishes with
/// the highest mapped code (0 with no generator).
///
/// **Mapping, applied to each code before the maximum:** 0..=255 is kept as
/// is; a negative code and a code above 255 both become 255. So a generator
/// returning -1 makes the run exit with 255 even when another returns 0, and
/// [`ExitStatus::code`](crate::ExitStatus::code) of the run is always in
/// 0..=255.
/// A forced exit by a second signal keeps its fixed code (130 or 143) and asks
/// no generator. Generators are collected before the container shuts down, so a
/// generator whose creation fails fails the run in the `Container` phase.
///
/// Generator beans are collected before the services start, so a lazy
/// generator bean is created then, not at exit time. A panic inside
/// [`exit_code`](ExitCodeGenerator::exit_code) propagates out of `run`, after
/// the services stopped and the container was shut down.
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_boot::{ExitCodeGenerator, OxbloomApplication};
/// use oxbloom_config::Config;
///
/// struct Degraded;
///
/// impl ExitCodeGenerator for Degraded {
///     fn exit_code(&self) -> i32 {
///         3
///     }
/// }
///
/// # tokio::runtime::Builder::new_current_thread().build()?.block_on(async {
/// let status = OxbloomApplication::builder()
///     .with_config(Config::from_str("")?)
///     .output(std::io::sink())
///     .configure(|b| {
///         b.instance(Degraded)
///             .provides(|d: Arc<Degraded>| -> Arc<dyn ExitCodeGenerator> { d })
///     })
///     .run()
///     .await?;
/// assert_eq!(status.code(), 3);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # })?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// An out-of-range code maps to 255 before the maximum is taken:
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_boot::{ExitCodeGenerator, OxbloomApplication};
/// use oxbloom_config::Config;
///
/// struct Failed;
/// impl ExitCodeGenerator for Failed {
///     fn exit_code(&self) -> i32 {
///         -1
///     }
/// }
/// struct Fine;
/// impl ExitCodeGenerator for Fine {
///     fn exit_code(&self) -> i32 {
///         0
///     }
/// }
///
/// # tokio::runtime::Builder::new_current_thread().build()?.block_on(async {
/// let status = OxbloomApplication::builder()
///     .with_config(Config::from_str("")?)
///     .output(std::io::sink())
///     .configure(|b| {
///         b.instance(Failed)
///             .provides(|f: Arc<Failed>| -> Arc<dyn ExitCodeGenerator> { f })
///             .instance(Fine)
///             .provides(|f: Arc<Fine>| -> Arc<dyn ExitCodeGenerator> { f })
///     })
///     .run()
///     .await?;
/// assert_eq!(status.code(), 255);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # })?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub trait ExitCodeGenerator: Send + Sync + 'static {
    /// The exit code this generator asks for.
    ///
    /// ```
    /// use oxbloom_boot::ExitCodeGenerator;
    ///
    /// struct Fixed;
    /// impl ExitCodeGenerator for Fixed {
    ///     fn exit_code(&self) -> i32 {
    ///         2
    ///     }
    /// }
    /// assert_eq!(Fixed.exit_code(), 2);
    /// ```
    fn exit_code(&self) -> i32;
}

/// Maps a generator's code to a process exit code: 0..=255 unchanged,
/// anything else (negative or above 255) becomes 255.
pub(crate) fn process_code(code: i32) -> i32 {
    if (0..=255).contains(&code) { code } else { 255 }
}

/// Clamps `code` into the range a process can exit with. Codes computed by
/// `run` are already in range; this covers statuses built with
/// `ExitStatus::new`.
pub(crate) fn clamp_code(code: i32) -> u8 {
    code.clamp(0, 255) as u8
}

pub(crate) fn to_exit_code(code: i32) -> ExitCode {
    ExitCode::from(clamp_code(code))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_code_maps_out_of_range_to_255() {
        for (code, mapped) in [
            (-1, 255),
            (i32::MIN, 255),
            (256, 255),
            (i32::MAX, 255),
            (0, 0),
            (42, 42),
            (255, 255),
        ] {
            assert_eq!(process_code(code), mapped, "{code}");
        }
    }

    #[test]
    fn negative_code_clamps_to_zero() {
        assert_eq!(clamp_code(-1), 0);
        assert_eq!(clamp_code(i32::MIN), 0);
    }

    #[test]
    fn code_above_255_clamps() {
        assert_eq!(clamp_code(256), 255);
        assert_eq!(clamp_code(i32::MAX), 255);
        assert_eq!(clamp_code(255), 255);
        assert_eq!(clamp_code(42), 42);
    }

    #[test]
    fn to_exit_code_uses_the_clamp() {
        for (code, n) in [(-5, 0u8), (0, 0), (7, 7), (300, 255)] {
            assert_eq!(
                format!("{:?}", to_exit_code(code)),
                format!("{:?}", ExitCode::from(n))
            );
        }
    }
}
