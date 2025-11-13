use ahash::AHashMap;
use std::collections::HashMap;

/// Prefix used by BMS files to mark section headers.
pub const BMS_FIELD_PREFIX: &str = "*---------------------- ";

/// BMS section kinds.
#[derive(Debug)]
pub enum BmsField {
    /// Header section (metadata, tables, etc.).
    Header,
    /// Main data section (timeline messages and objects).
    Data,
    /// Unknown or unsupported section.
    Unknown,
}

impl BmsField {
    /// Parse a section header line into a `BmsField`.
    ///
    /// # Arguments
    ///
    /// * `s` - A full line from the BMS file.
    ///
    /// # Returns
    ///
    /// * `BmsField` - Parsed section kind or `Unknown`.
    pub fn parse(s: &str) -> BmsField {
        if !s.starts_with(BMS_FIELD_PREFIX) {
            return BmsField::Unknown;
        }

        let s = &s[BMS_FIELD_PREFIX.len()..];
        match s {
            "HEADER FIELD" => BmsField::Header,
            "MAIN DATA FIELD" => BmsField::Data,
            _ => BmsField::Unknown,
        }
    }
}

/// Parsed BMS chart containing header and timeline messages.
#[derive(Debug, Default)]
pub struct Bms {
    /// Parsed header metadata and lookup tables.
    pub header: Header,
    /// Timeline messages (per-measure, per-channel, with objects).
    pub messages: Vec<Message>,
    /// Per-measure length multipliers (e.g., for measure length changes).
    pub measure_multipliers: AHashMap<u16, f64>,
}

impl Bms {
    /// Parse a BMS file content into a `Bms` structure.
    ///
    /// # Arguments
    ///
    /// * `data` - Full text content of a BMS file.
    ///
    /// # Returns
    ///
    /// * `Result<Bms, ParseError>` - Parsed chart or an error.
    pub fn parse(data: &str) -> Result<Self, ParseError> {
        let mut bms = Bms::default();
        let mut current_field = BmsField::Unknown;

        for line in data.lines() {
            let line = line.trim();

            if line.starts_with(BMS_FIELD_PREFIX) {
                current_field = BmsField::parse(line);
                continue;
            }

            match current_field {
                BmsField::Header => bms.header.parse_line(line),
                BmsField::Data => {
                    if line.starts_with('#') && line.len() >= 7 {
                        let mmm = &line[1..4];
                        let cc = &line[4..6];
                        if cc.eq_ignore_ascii_case("02")
                            && let Some((_hash, rest)) = line.split_once(':')
                        {
                            if let Ok(measure) = mmm.parse::<u16>() {
                                let v = rest.trim();
                                if let Ok(mult) = v.parse::<f64>()
                                    && mult.is_finite()
                                    && mult > 0.0
                                {
                                    bms.measure_multipliers.insert(measure, mult);
                                }
                            }
                            continue;
                        }
                    }
                    if let Ok(message) = Message::parse(line) {
                        bms.messages.push(message);
                    }
                }
                BmsField::Unknown => continue,
            }
        }
        Ok(bms)
    }
}

/// Header metadata and lookup tables of a BMS chart.
#[derive(Debug, Default)]
pub struct Header {
    /// Player mode.
    pub player: Option<u8>,
    /// Music genre.
    pub genre: Option<String>,
    /// Song title.
    pub title: Option<String>,
    /// Song artist.
    pub artist: Option<String>,
    /// Base BPM.
    pub bpm: f64,
    /// Displayed difficulty level.
    pub play_level: Option<u8>,
    /// Ranking setting.
    pub rank: Option<u8>,
    /// Stage background file path.
    pub stage_file: Option<String>,
    /// Banner image path.
    pub banner: Option<String>,
    /// Difficulty code.
    pub difficulty: Option<u8>,
    /// Gauge total value.
    pub total: Option<f64>,
    /// Long note handling type.
    pub ln_type: Option<u8>,
    /// Long note end object id.
    pub ln_obj: Option<String>,
    /// Mapping from object id to audio filename.
    pub audio_files: HashMap<String, String>,
    /// Mapping from BPM id to BPM value.
    pub bpm_table: HashMap<String, f64>,
    /// Mapping from STOP id to stop duration.
    pub stop_table: HashMap<String, f64>,
}

impl Header {
    /// Parse a single header line and update fields as needed.
    ///
    /// # Arguments
    ///
    /// * `line` - A header line starting with `#`.
    fn parse_line(&mut self, line: &str) {
        if !line.starts_with('#') {
            return;
        }

        let parts: Vec<&str> = line[1..].splitn(2, ' ').collect();
        if parts.len() < 2 {
            return;
        }

        let key = parts[0].to_uppercase();
        let value = parts[1].trim().trim_matches('"');

        match key.as_str() {
            "PLAYER" => self.player = value.parse().ok(),
            "GENRE" => self.genre = Some(value.to_string()),
            "TITLE" => self.title = Some(value.to_string()),
            "ARTIST" => self.artist = Some(value.to_string()),
            "BPM" => self.bpm = value.parse().unwrap_or(120.0),
            "PLAYLEVEL" => self.play_level = value.parse().ok(),
            "RANK" => self.rank = value.parse().ok(),
            "STAGEFILE" => self.stage_file = Some(value.to_string()),
            "BANNER" => self.banner = Some(value.to_string()),
            "DIFFICULTY" => self.difficulty = value.parse().ok(),
            "TOTAL" => self.total = value.parse().ok(),
            "LNTYPE" => self.ln_type = value.parse().ok(),
            "LNOBJ" => self.ln_obj = Some(value.to_uppercase()),
            _ if key.starts_with("WAV") || key.starts_with("OGG") => {
                let audio_id = key[3..].to_string();
                self.audio_files.insert(audio_id, value.to_string());
            }
            _ if key.starts_with("BPM") && key.len() > 3 => {
                let bpm_id = key[3..].to_string();
                if let Ok(bpm_value) = value.parse::<f64>()
                    && bpm_value.is_finite()
                    && bpm_value > 0.0
                {
                    self.bpm_table.insert(bpm_id.to_uppercase(), bpm_value);
                }
            }
            _ if key.starts_with("STOP") => {
                let stop_id = key[4..].to_string();
                if let Ok(stop_value) = value.parse::<f64>()
                    && stop_value.is_finite()
                    && stop_value >= 0.0
                {
                    self.stop_table.insert(stop_id.to_uppercase(), stop_value);
                }
            }
            _ => (),
        }
    }
}

/// Errors that can occur while parsing BMS data.
#[derive(Debug)]
pub enum ParseError {
    /// The line format was invalid.
    InvalidFormat,
    /// Failed to parse the measure field.
    InvalidMeasure(std::num::ParseIntError),
    /// Failed to parse the channel field.
    InvalidChannel(std::num::ParseIntError),
    /// Object data was malformed.
    InvalidObjectData,
}

impl core::fmt::Display for ParseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ParseError::InvalidFormat => write!(f, "invalid BMS line format"),
            ParseError::InvalidMeasure(e) => write!(f, "invalid measure: {}", e),
            ParseError::InvalidChannel(e) => write!(f, "invalid channel: {}", e),
            ParseError::InvalidObjectData => {
                write!(f, "invalid object data (must be pairs of two chars)")
            }
        }
    }
}

impl std::error::Error for ParseError {}

/// A per-measure, per-channel message with a list of 2-char object tokens.
#[derive(Debug, Clone)]
pub struct Message {
    /// Measure index of this message.
    pub measure: u16,
    /// Channel identifier.
    pub channel: u8,
    /// Objects appearing in this message line.
    pub objects: Vec<Object>,
}

impl Message {
    /// Parse a message line.
    ///
    /// # Arguments
    ///
    /// * `data` - A full message line.
    ///
    /// # Returns
    ///
    /// * `Result<Message, ParseError>` - Parsed message or an error.
    pub fn parse(data: &str) -> Result<Self, ParseError> {
        if !data.starts_with('#') || data.len() < 7 || !data.contains(':') {
            return Err(ParseError::InvalidFormat);
        }

        let measure_str = &data[1..4];
        let channel_str = &data[4..6];
        let objects_str = match data.split_once(':') {
            Some((_, objects)) => objects,
            None => return Err(ParseError::InvalidFormat),
        };

        let measure: u16 = measure_str.parse().map_err(ParseError::InvalidMeasure)?;
        let channel: u8 = u8::from_str_radix(channel_str, 36)
            .unwrap_or_else(|_| channel_str.parse().unwrap_or(0));

        if objects_str.len() % 2 != 0 {
            return Err(ParseError::InvalidObjectData);
        }

        let mut objects: Vec<Object> = Vec::with_capacity(objects_str.len() / 2);
        for chunk in objects_str.as_bytes().chunks(2) {
            let s = std::str::from_utf8(chunk).map_err(|_| ParseError::InvalidObjectData)?;
            objects.push(Object(s.to_string()));
        }

        Ok(Message {
            measure,
            channel,
            objects,
        })
    }
}

/// A 2-character object token.
#[derive(Debug, Clone)]
pub struct Object(pub String);

impl std::ops::Deref for Object {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
