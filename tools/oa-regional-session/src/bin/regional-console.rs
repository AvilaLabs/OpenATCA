#![forbid(unsafe_code)]
//! Dedicated single-thread operated recorded-session console. All timings remain explicit replay
//! cutoffs or input journal evidence; this is not a live surveillance or operational console.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result =
        oa_regional_session::operator::parse_args(&args).and_then(|options| match options.mode {
            oa_regional_session::operator::ConsoleMode::Script(script) => {
                oa_regional_session::operator::run_script_session(
                    &options.input,
                    &options.output,
                    &options.model,
                    &script,
                )
            }
            oa_regional_session::operator::ConsoleMode::Device {
                drm,
                tty,
                connector,
                input_device,
                idle_limit_seconds,
            } => oa_regional_session::operator::run_device_session(
                &options.input,
                &options.output,
                &options.model,
                &oa_regional_session::operator::NativeConsoleConfig {
                    drm: &drm,
                    tty: &tty,
                    connector,
                    input_device: &input_device,
                    idle_limit_seconds,
                },
            ),
        });
    if let Err(error) = result {
        eprintln!("oa-regional-console: {error}");
        std::process::exit(2);
    }
}
