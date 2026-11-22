use super::{Lyrics, Record, TrackKey, best};
use serde::de::DeserializeOwned;
use std::{
    hash::{BuildHasher, RandomState},
    thread,
    time::{Duration, Instant},
};
use url::Url;

const SPACING: Duration = Duration::from_millis(300);

pub(crate) struct Client {
    agent: ureq::Agent,
    last_request: Option<Instant>,
    backoff: Duration,
    base: Url,
}

impl Client {
    pub fn new() -> Self {
        let config = ureq::Agent::config_builder()
            .user_agent(concat!(
                "MediaFlyout v",
                env!("CARGO_PKG_VERSION"),
                " (",
                env!("CARGO_PKG_REPOSITORY"),
                ")"
            ))
            .timeout_global(Some(Duration::from_secs(12)))
            .http_status_as_error(false)
            .max_redirects(0)
            .build();
        Self {
            agent: config.into(),
            last_request: None,
            backoff: Duration::ZERO,
            base: Url::parse("https://lrclib.net/api/").expect("LRCLIB base URL"),
        }
    }

    fn request<T: DeserializeOwned>(&mut self, url: Url) -> Result<Option<T>, String> {
        if let Some(last) = self.last_request {
            let wait = self.backoff.saturating_sub(last.elapsed());
            if !wait.is_zero() {
                return Err(wait_message(wait));
            }
            self.backoff = Duration::ZERO;
            thread::sleep(SPACING.saturating_sub(last.elapsed()));
        }
        let response = self.agent.get(url.as_str()).call();
        self.last_request = Some(Instant::now());
        let mut response =
            response.map_err(|_| "Couldn’t reach LRCLIB. Reopen Lyrics to retry.".to_owned())?;
        match response.status().as_u16() {
            200 => {}
            404 => return Ok(None),
            code @ (429 | 503) => {
                let seconds = response
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.trim().parse::<u64>().ok())
                    .unwrap_or(if code == 429 { 60 } else { 1 })
                    .max(1);
                let jitter_ms = RandomState::new().hash_one(seconds) % (seconds.min(10) * 300 + 1);
                self.backoff =
                    Duration::from_secs(seconds).saturating_add(Duration::from_millis(jitter_ms));
                return Err(wait_message(self.backoff));
            }
            code => {
                return Err(format!(
                    "LRCLIB returned HTTP {code}. Reopen Lyrics to retry."
                ));
            }
        }
        let json = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.split(';').next())
            .is_some_and(|s| s.trim().eq_ignore_ascii_case("application/json"));
        if !json {
            return Err("LRCLIB returned an unexpected response.".into());
        }
        let bytes = response
            .body_mut()
            .with_config()
            .limit(4 * 1024 * 1024)
            .read_to_vec()
            .map_err(|_| "Couldn’t read the lyrics response.".to_owned())?;
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| "Couldn’t read LRCLIB’s lyrics format.".into())
    }

    pub fn lookup(&mut self, key: &TrackKey) -> Result<Option<Lyrics>, String> {
        let url = |path: &str| {
            let mut url = self.base.join(path).expect("static endpoint");
            url.query_pairs_mut()
                .append_pair("track_name", &key.title)
                .append_pair("artist_name", &key.artist);
            url
        };
        let search = url("search");
        let mut exact = url("get");
        if !key.album.is_empty() {
            exact
                .query_pairs_mut()
                .append_pair("album_name", &key.album);
        }
        if let Some(duration @ 1..=3600) = key.duration {
            exact
                .query_pairs_mut()
                .append_pair("duration", &duration.to_string());
        }
        // Stable v1 search includes every representation, so rank actual usable
        // timings rather than trusting summary flags or fetching every candidate.
        let matches: Vec<Record> = self.request(search)?.unwrap_or_default();
        if let Some(lyrics) = best(key, matches) {
            return Ok(Some(lyrics));
        }
        Ok(self
            .request::<Record>(exact)?
            .and_then(|r| best(key, vec![r])))
    }
}

fn wait_message(wait: Duration) -> String {
    let seconds = wait
        .as_secs()
        .saturating_add(u64::from(wait.subsec_nanos() > 0));
    format!("LRCLIB is busy. Reopen Lyrics in {seconds} s.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
    };

    #[test]
    fn non_json_rate_limit_blocks_other_tracks_too() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut client = Client::new();
        client.base =
            Url::parse(&format!("http://{}/api/", listener.local_addr().unwrap())).unwrap();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let headers = BufReader::new(&socket)
                .lines()
                .map(Result::unwrap)
                .take_while(|line| !line.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            assert!(headers.contains("track_name=A%26B"));
            assert!(headers.contains("MediaFlyout v"));
            socket.write_all(b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 90\r\nContent-Type: text/plain\r\nContent-Length: 4\r\n\r\nbusy").unwrap();
        });
        let mut key = TrackKey {
            title: "A&B".into(),
            artist: "Artist".into(),
            album: String::new(),
            duration: None,
        };
        assert!(
            client
                .lookup(&key)
                .unwrap_err()
                .contains("Reopen Lyrics in")
        );
        key.title = "Other".into();
        assert!(
            client
                .lookup(&key)
                .unwrap_err()
                .contains("Reopen Lyrics in")
        );
        assert!(client.backoff >= Duration::from_secs(90));
        server.join().unwrap();
    }

    #[test]
    #[ignore = "Queries LRCLIB using its public documentation example"]
    fn live_lookup() {
        let key = TrackKey {
            title: "I Want to Live".into(),
            artist: "Borislav Slavov".into(),
            album: "Baldur's Gate 3 (Original Game Soundtrack)".into(),
            duration: Some(233),
        };
        let lyrics = Client::new()
            .lookup(&key)
            .unwrap()
            .expect("documentation example track");
        assert!(!lyrics.plain.trim().is_empty());
    }
}
