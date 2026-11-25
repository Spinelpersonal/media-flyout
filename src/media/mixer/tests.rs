use super::*;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::System::Com::{CoCreateGuid, CoTaskMemFree};

#[test]
fn desktop_identity_needs_the_exact_image_or_registered_app_id() {
    let _apartment = Apartment::new().unwrap();
    let exe = std::env::current_exe().unwrap();
    let filename = exe.file_name().unwrap().to_str().unwrap();
    for id in [filename, exe.to_str().unwrap()] {
        let source = Source::new(id);
        assert!(source.owns(std::process::id()));
    }
    for id in [
        format!(r"C:\another installation\{filename}"),
        "Media player".into(),
        "Other.Package!Player".into(),
    ] {
        let source = Source::new(&id);
        assert!(!source.owns(std::process::id()));
    }
}

/// Open a fresh nonpersistent playback session owned by this test executable.
/// The client stays alive for the test and submits no audible samples.
fn silent_session(device: &IMMDevice) -> Result<(IAudioClient, ISimpleAudioVolume)> {
    // SAFETY: All interfaces belong to the test's apartment. Initialize copies the
    // mix format and the new GUID. Release the format regardless of success.
    unsafe {
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
        let guid = CoCreateGuid()?;
        let format = client.GetMixFormat()?;
        let initialized = client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_NOPERSIST,
            1_000_000,
            0,
            format,
            Some(&guid),
        );
        CoTaskMemFree(Some(format.cast()));
        initialized?;
        let volume = client.GetService()?;
        client.Start()?;
        Ok((client, volume))
    }
}

#[test]
#[ignore = "Requires a Windows render endpoint. Changes only this test executable's private sessions."]
fn live_application_mixer() {
    let _apartment = Apartment::new().unwrap();
    // SAFETY: The test owns this apartment. Read the default output's master level for comparison.
    let (device, alternate, master, initial_master) = unsafe {
        let devices: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).unwrap();
        let device = devices
            .GetDefaultAudioEndpoint(eRender, eMultimedia)
            .unwrap();
        let master: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None).unwrap();
        let initial_master = master.GetMasterVolumeLevelScalar().unwrap();
        let id = take_string(device.GetId().unwrap()).unwrap();
        let outputs = devices
            .EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)
            .unwrap();
        let alternate = (0..outputs.GetCount().unwrap())
            .filter_map(|i| outputs.Item(i).ok())
            .find(|output| {
                output
                    .GetId()
                    .ok()
                    .and_then(|value| take_string(value))
                    .is_some_and(|other| other != id)
            });
        (device, alternate, master, initial_master)
    };
    let (client, external) = silent_session(&device).unwrap();
    let app = std::env::current_exe()
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let (tx, rx) = mpsc::channel();
    let mixer = Mixer::start((1, app), move || {
        let _ = tx.send(());
    })
    .unwrap();
    let update = || {
        rx.recv_timeout(Duration::from_secs(5))
            .expect("mixer update");
        mixer.state()
    };
    let binding = update().expect("unique session").instance;
    // SAFETY: external controls only our isolated session. These operations simulate SndVol.
    unsafe {
        external.SetMasterVolume(0.36, std::ptr::null()).unwrap();
        assert_eq!(update().unwrap().percent, 36);
        mixer.change(binding.clone(), Change::Step(5));
        assert_eq!(update().unwrap().percent, 41);
        assert!((external.GetMasterVolume().unwrap() - 0.41).abs() < 0.001);
        mixer.change(binding.clone(), Change::ToggleMute);
        assert!(update().unwrap().muted && external.GetMute().unwrap().as_bool());
        external.SetMute(false, std::ptr::null()).unwrap();
        assert!(!update().unwrap().muted);

        mixer.change("superseded instance".into(), Change::Set(99));
        // The worker processes requests in order. This step would reach 100 after a stale write.
        mixer.change(binding.clone(), Change::Step(5));
        assert_eq!(update().unwrap().percent, 46);
        let second_output = alternate.as_ref().unwrap_or(&device);
        let (_second, other) = silent_session(second_output).unwrap();
        println!(
            "Exercising session replacement across {} output device(s)",
            if alternate.is_some() { 2 } else { 1 }
        );
        let other_level = other.GetMasterVolume().unwrap();
        mixer.change(binding.clone(), Change::Set(5));
        assert!(
            update().is_none(),
            "two matching sessions disable the mixer"
        );
        assert_eq!(other.GetMasterVolume().unwrap(), other_level);
        assert!((external.GetMasterVolume().unwrap() - 0.46).abs() < 0.001);
        drop(external);
        drop(client);
        let replacement = update().expect("rediscover the surviving session");
        assert_ne!(replacement.instance, binding);
        mixer.change(binding, Change::Set(5));
        mixer.change(replacement.instance, Change::Set(63));
        assert_eq!(update().unwrap().percent, 63);
        assert!((other.GetMasterVolume().unwrap() - 0.63).abs() < 0.001);
        assert_eq!(master.GetMasterVolumeLevelScalar().unwrap(), initial_master);
    }
}
