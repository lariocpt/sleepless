use anyhow::Result;
use clap::Parser;
use sleepless::{app::App, cli, config, inhibit, power, run};
use std::time::Duration;

fn main() -> Result<()> {
    let args = cli::Args::parse();
    let config::Loaded { file, mut warnings } = config::load();

    let state = if args.reset_state {
        if let Err(e) = config::clear_state() {
            warnings.push(format!("state: could not reset: {e}"));
        }
        config::State::default()
    } else {
        config::load_state()
    };

    let settings = cli::resolve(&args, &file, &state, warnings);
    let buses = inhibit::Buses::connect();
    let mut app = App::new(
        settings.mode,
        settings.lid,
        settings.pulse,
        settings.why.clone(),
    );
    app.splashes = settings.splashes;
    app.splash_idx = settings.splash_idx;
    app.config_notes = settings.warnings;
    app.power = power::read();
    app.reconcile(&buses);

    match args.smoke {
        Some(secs) => {
            run::print_state(&app);
            run::plain(&mut app, &buses, Duration::from_secs(secs))
        }
        None => run::tui(&mut app, &buses, settings.tray),
    }
}
