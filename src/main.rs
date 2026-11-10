#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use media_flyout::{config::ConfigStore, media, ui::RunOptions};
use std::{ffi::OsString, path::PathBuf, time::Duration};

#[derive(Default)]
struct Arguments {
    command: Option<String>,
    config: Option<PathBuf>,
    preview: Option<PathBuf>,
}

impl Arguments {
    fn parse(arguments: impl IntoIterator<Item = OsString>) -> Result<Self, String> {
        let mut result = Self::default();
        let mut arguments = arguments.into_iter();
        while let Some(argument) = arguments.next() {
            if argument == "--config" {
                if result.config.is_some() {
                    return Err("--config may only be supplied once".into());
                }
                result.config = Some(
                    arguments
                        .next()
                        .filter(|p| !p.is_empty())
                        .ok_or("--config requires a file path")?
                        .into(),
                );
                continue;
            }
            let argument = argument.into_string().map_err(|_| "Invalid argument")?;
            if result.command.is_some() {
                return Err("Supply only one command".into());
            }
            match argument.as_str() {
                "--preview" => {
                    result.preview = Some(
                        arguments
                            .next()
                            .filter(|p| !p.is_empty())
                            .ok_or("--preview requires an output .bmp path")?
                            .into(),
                    );
                }
                "--show" | "--quit" | "--reload" | "--diagnose" | "--watch" | "--play-pause"
                | "--next" | "--previous" | "--help" | "-h" | "--version" => {}
                _ => {
                    return Err(format!(
                        "Unknown argument: {argument}. Use --help for usage."
                    ));
                }
            }
            result.command = Some(argument);
        }
        Ok(result)
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("Media Flyout: {error}");
        if std::env::args_os().len() == 1 {
            media_flyout::platform::show_error(&error.to_string());
        }
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let arguments = Arguments::parse(std::env::args_os().skip(1))?;
    let command = arguments.command.as_deref().unwrap_or("");
    match command {
        "" | "--show" | "--preview" => {
            return media_flyout::ui::run(RunOptions {
                show: !command.is_empty(),
                preview: arguments.preview,
                config_path: arguments.config,
            });
        }
        "--quit" => {
            media_flyout::platform::request_exit()?;
            return Ok(());
        }
        "--reload" => {
            media_flyout::platform::request_reload()?;
            return Ok(());
        }
        "--version" => {
            println!("Media Flyout {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        "--help" | "-h" => {
            println!(
                "Media Flyout\n\nStart without arguments to run in the tray. Ctrl+Space toggles the flyout.\n\n  --show                 Show the running flyout, or start it visible\n  --quit                 Exit the running flyout\n  --reload               Reload the running flyout's settings\n  --config <path>        Use a specific TOML file\n  --diagnose             Print a snapshot of real Windows media sessions\n  --watch                Print event-driven media updates until interrupted\n  --play-pause           Send play/pause to the selected session\n  --previous / --next    Skip tracks in the selected session\n  --preview <path.bmp>   Save a rendered preview and exit (close other instances first)\n  --version             Print the version"
            );
            return Ok(());
        }
        _ => {}
    }
    let config = ConfigStore::load(arguments.config)?;
    for warning in &config.warnings {
        eprintln!("{warning}");
    }
    let (sender, receiver) = std::sync::mpsc::channel();
    let service =
        media::MediaService::with_preference(config.settings.preferred_app, move |snapshot| {
            let _ = sender.send(snapshot);
        })?;
    match command {
        "--diagnose" => println!("{:#?}", receiver.recv_timeout(Duration::from_secs(15))?),
        "--watch" => {
            while let Ok(snapshot) = receiver.recv() {
                println!("{snapshot:#?}");
            }
        }
        "--play-pause" | "--next" | "--previous" => {
            receiver.recv_timeout(Duration::from_secs(15))?;
            let action = match command {
                "--next" => media::Action::Next,
                "--previous" => media::Action::Previous,
                _ => media::Action::PlayPause,
            };
            service.control(action);
            if let Ok(snapshot) = receiver.recv_timeout(Duration::from_secs(10)) {
                println!("{snapshot:#?}");
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse(args: &[&str]) -> Result<Arguments, String> {
        Arguments::parse(args.iter().map(OsString::from))
    }
    #[test]
    fn accepts_config_on_either_side_of_command() {
        assert_eq!(
            parse(&["--config", "a b.toml", "--show"])
                .expect("arguments")
                .config,
            Some(PathBuf::from("a b.toml"))
        );
        assert_eq!(
            parse(&["--show", "--config", "a.toml"])
                .expect("arguments")
                .command
                .as_deref(),
            Some("--show")
        );
    }
    #[test]
    fn rejects_missing_paths_unknown_and_multiple_commands() {
        for args in [
            &["--config"][..],
            &["--preview"],
            &["--show", "--quit"],
            &["--unknown"],
            &["--config", "x", "--config", "y"],
        ] {
            assert!(parse(args).is_err());
        }
    }
}
