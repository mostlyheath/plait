//! Internal helper; shared headless code owns policy and recoverable filesystem work.
pub fn run() -> std::io::Result<()> {
    plait_engine::build_output::store::run()
}
