mod client;
pub(crate) use client::Client;

use serde::Deserialize;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TrackKey {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Record {
    track_name: String,
    artist_name: String,
    album_name: String,
    duration: f64,
    instrumental: bool,
    lyricsfile: Option<String>,
    synced_lyrics: Option<String>,
    plain_lyrics: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Word {
    pub text: String,
    start_ms: u64,
    end_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Line {
    pub text: String,
    start_ms: u64,
    end_ms: Option<u64>,
    #[serde(default)]
    pub words: Vec<Word>,
}

#[derive(Deserialize)]
struct Lyricsfile {
    version: String,
    #[serde(default)]
    lines: Vec<Line>,
    plain: Option<String>,
}

#[derive(Debug)]
pub(crate) struct Lyrics {
    pub lines: Vec<Line>,
    pub plain: String,
    pub instrumental: bool,
}

impl Lyrics {
    fn decode(record: Record) -> Option<Self> {
        let file = record
            .lyricsfile
            .as_deref()
            .and_then(|yaml| serde_yaml_ng::from_str::<Lyricsfile>(yaml).ok())
            .filter(|file| file.version == "1.0");
        let mut lines = Vec::new();
        let mut plain = record.plain_lyrics.unwrap_or_default();
        if let Some(file) = file {
            lines = file.lines;
            if let Some(text) = file.plain.filter(|s| !s.trim().is_empty()) {
                plain = text;
            }
            lines.retain(|l| l.end_ms.is_none_or(|end| end >= l.start_ms));
            for line in &mut lines {
                if !line.words.is_sorted_by_key(|w| w.start_ms)
                    || line
                        .words
                        .iter()
                        .any(|w| w.end_ms.is_some_and(|end| end < w.start_ms))
                    || !line
                        .words
                        .iter()
                        .flat_map(|w| w.text.bytes())
                        .eq(line.text.bytes())
                {
                    line.words.clear();
                }
            }
        }
        if lines.iter().all(|l| l.text.trim().is_empty()) {
            lines = parse_lrc(record.synced_lyrics.as_deref().unwrap_or_default());
        }
        lines.sort_by_key(|l| l.start_ms);
        if plain.trim().is_empty() {
            plain = lines
                .iter()
                .map(|l| l.text.as_str())
                .collect::<Vec<_>>()
                .join("\n");
        }
        (record.instrumental || !plain.trim().is_empty()).then_some(Self {
            lines,
            plain,
            instrumental: record.instrumental,
        })
    }

    fn quality(&self) -> u8 {
        if self.lines.iter().any(|l| !l.words.is_empty()) {
            3
        } else if !self.lines.is_empty() {
            2
        } else if self.instrumental {
            0
        } else {
            1
        }
    }

    /// Index of the current line, active word, and following line. During
    /// an instrumental gap, keep the upcoming line available as a preview.
    pub fn cursor(&self, ms: u64) -> (Option<usize>, Option<usize>, Option<usize>) {
        let next = self.lines.partition_point(|l| l.start_ms <= ms);
        let active = self.lines[..next].iter().rposition(|l| {
            l.end_ms
                .map_or(l.start_ms == self.lines[next - 1].start_ms, |end| ms < end)
        });
        let word = active.and_then(|i| {
            let words = &self.lines[i].words;
            let index = words.partition_point(|w| w.start_ms <= ms).checked_sub(1)?;
            words[index]
                .end_ms
                .is_none_or(|end| ms < end)
                .then_some(index)
        });
        (active, word, (next < self.lines.len()).then_some(next))
    }
}

fn timestamp(s: &str) -> Option<u64> {
    let (m, s) = s.split_once(':')?;
    let (s, fraction) = s.split_once('.').unwrap_or((s, ""));
    let seconds = s.parse::<u64>().ok().filter(|s| *s < 60)?;
    if !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let millis = fraction
        .bytes()
        .take(3)
        .zip([100, 10, 1])
        .map(|(b, scale)| u64::from(b - b'0') * scale)
        .sum::<u64>();
    m.parse::<u64>()
        .ok()?
        .checked_mul(60000)?
        .checked_add(seconds * 1000 + millis)
}

fn parse_lrc(text: &str) -> Vec<Line> {
    let mut lines = Vec::new();
    for row in text.lines() {
        let mut rest = row.trim();
        let mut times = Vec::new();
        while let Some((tag, tail)) = rest.strip_prefix('[').and_then(|s| s.split_once(']')) {
            let Some(t) = timestamp(tag) else {
                break;
            };
            times.push(t);
            rest = tail.trim_start();
        }
        // Multiple timestamps may repeat one refrain. Bound expansion of malformed input.
        for start_ms in times.into_iter().take(64) {
            lines.push(Line {
                text: rest.into(),
                start_ms,
                end_ms: None,
                words: Vec::new(),
            });
        }
    }
    lines
}

fn same(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

fn best(key: &TrackKey, records: Vec<Record>) -> Option<Lyrics> {
    records
        .into_iter()
        .filter_map(|r| {
            if !same(&key.title, &r.track_name) || !same(&key.artist, &r.artist_name) {
                return None;
            }
            let difference = key.duration.map_or(0., |d| (d as f64 - r.duration).abs());
            if difference > 2.0 {
                return None;
            }
            let album_match = !key.album.is_empty() && same(&key.album, &r.album_name);
            if !album_match && !key.album.is_empty() && key.duration.is_none() {
                return None;
            }
            let lyrics = Lyrics::decode(r)?;
            Some((
                (
                    album_match,
                    lyrics.quality(),
                    std::cmp::Reverse((difference * 1000.) as u64),
                ),
                lyrics,
            ))
        })
        .max_by_key(|(score, _)| *score)
        .map(|(_, lyrics)| lyrics)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORDS: &str = "version: '1.0'\nlines:\n  - text: 'One two'\n    start_ms: 200\n    end_ms: 1000\n    words: [{text: 'One ', start_ms: 200, end_ms: 500}, {text: two, start_ms: 600}]\n";

    fn record(yaml: &str, synced: &str) -> Record {
        Record {
            track_name: "Song".into(),
            artist_name: "Artist".into(),
            album_name: "Album".into(),
            duration: 120.,
            lyricsfile: Some(yaml.into()),
            synced_lyrics: Some(synced.into()),
            plain_lyrics: Some("One two".into()),
            ..Record::default()
        }
    }

    #[test]
    fn prioritizes_actual_timing_quality_for_the_recording() {
        let key = TrackKey {
            title: "song".into(),
            artist: "ARTIST".into(),
            album: "Album".into(),
            duration: Some(120),
        };
        let word = || record(WORDS, "");
        let line = || record("", "[00:00.20][00:03.500]One two");
        let plain = || record("version: '2.0'", "");
        assert!(
            best(
                &key,
                vec![Record {
                    duration: 150.,
                    ..word()
                }]
            )
            .is_none()
        );
        assert_eq!(
            best(&key, vec![word(), line(), plain()]).unwrap().quality(),
            3
        );
        let lyrics = best(
            &key,
            vec![
                Record {
                    album_name: "Other".into(),
                    ..word()
                },
                line(),
                plain(),
            ],
        )
        .unwrap();
        assert_eq!(lyrics.quality(), 2);
        assert_eq!(lyrics.lines[1].start_ms, 3500);
        assert_eq!(best(&key, vec![plain()]).unwrap().quality(), 1);
    }

    #[test]
    fn follows_word_boundaries_gaps_and_backward_seeks() {
        let lyrics = Lyrics::decode(record(WORDS, "")).unwrap();
        assert_eq!(lyrics.cursor(199), (None, None, Some(0)));
        assert_eq!(lyrics.cursor(600), (Some(0), Some(1), None));
        assert_eq!(lyrics.cursor(1000), (None, None, None));
        assert_eq!(lyrics.cursor(500), (Some(0), None, None));
        assert_eq!(lyrics.cursor(200), (Some(0), Some(0), None));
    }
}
