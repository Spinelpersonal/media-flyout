use media_flyout::media;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let argument = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "--diagnose".into());
    let (sender, receiver) = std::sync::mpsc::channel();
    let media = media::MediaService::start(move |snapshot| {
        let _ = sender.send(snapshot);
    })?;
    match argument.as_str() {
        "--diagnose" => {
            let snapshot = receiver.recv_timeout(std::time::Duration::from_secs(15))?;
            println!("{snapshot:#?}");
        }
        "--watch" => {
            while let Ok(snapshot) = receiver.recv() {
                println!("{snapshot:#?}");
            }
        }
        "--play-pause" | "--next" | "--previous" => {
            let _ = receiver.recv_timeout(std::time::Duration::from_secs(15))?;
            let action = match argument.as_str() {
                "--next" => media::Action::Next,
                "--previous" => media::Action::Previous,
                _ => media::Action::PlayPause,
            };
            media.control(action);
            if let Ok(snapshot) = receiver.recv_timeout(std::time::Duration::from_secs(10)) {
                println!("{snapshot:#?}");
            }
        }
        _ => return Err(format!("Unknown argument: {argument}").into()),
    }
    drop(media);
    Ok(())
}
