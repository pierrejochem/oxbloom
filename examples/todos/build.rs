//! Sets the build facts that `oxbloom_actuator::build_info!` reads. It never fails
//! when git is missing.

fn main() {
    oxbloom_actuator::build::emit();
}
