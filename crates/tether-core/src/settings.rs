//! Stream settings (`[stream]` in `config.toml`) and the summary of the last
//! session (`[last_session]`).

use std::fmt;

use rp_core::{Codec, Resolution, VideoProfile};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum VideoResolution {
    #[serde(rename = "360p")]
    P360,
    #[serde(rename = "540p")]
    P540,
    #[default]
    #[serde(rename = "720p")]
    P720,
    #[serde(rename = "1080p")]
    P1080,
}

impl VideoResolution {
    pub const ALL: [Self; 4] = [Self::P360, Self::P540, Self::P720, Self::P1080];

    pub fn to_rp(self) -> Resolution {
        match self {
            Self::P360 => Resolution::P360,
            Self::P540 => Resolution::P540,
            Self::P720 => Resolution::P720,
            Self::P1080 => Resolution::P1080,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::P360 => "360p",
            Self::P540 => "540p",
            Self::P720 => "720p",
            Self::P1080 => "1080p",
        }
    }
}

impl fmt::Display for VideoResolution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoCodec {
    #[default]
    H264,
    H265,
}

impl VideoCodec {
    pub fn to_rp(self) -> Codec {
        match self {
            Self::H264 => Codec::H264,
            Self::H265 => Codec::H265,
        }
    }

    /// Name shown in the UI.
    pub fn label(self) -> &'static str {
        match self {
            Self::H264 => "H.264",
            Self::H265 => "HEVC",
        }
    }
}

/// Which video decoder to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DecoderPreference {
    /// Hardware (VAAPI on Linux, D3D11VA on Windows) when it works, else software.
    #[default]
    Auto,
    Software,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StreamSettings {
    pub resolution: VideoResolution,
    /// 30 or 60.
    pub fps: u32,
    pub codec: VideoCodec,
    /// kbit/s. `None` uses the preset for the resolution.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bitrate_kbps: Option<u32>,
    pub decoder: DecoderPreference,
}

impl Default for StreamSettings {
    fn default() -> Self {
        Self {
            resolution: VideoResolution::default(),
            fps: 60,
            codec: VideoCodec::default(),
            bitrate_kbps: None,
            decoder: DecoderPreference::default(),
        }
    }
}

/// Bitrates the settings accept, in kbit/s.
pub const BITRATE_RANGE_KBPS: (u32, u32) = (2_000, 50_000);

impl StreamSettings {
    /// 30 or 60, whatever was stored.
    pub fn fps(&self) -> u32 {
        if self.fps >= 60 { 60 } else { 30 }
    }

    /// What to ask the console for.
    pub fn video_profile(&self) -> VideoProfile {
        let mut profile =
            VideoProfile::preset(self.resolution.to_rp(), self.fps(), self.codec.to_rp());
        if let Some(kbps) = self.bitrate_kbps {
            profile.bitrate = kbps.clamp(BITRATE_RANGE_KBPS.0, BITRATE_RANGE_KBPS.1);
        }
        profile
    }
}

/// A short record of the previous session, for the consoles screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastSession {
    pub console: String,
    /// Seconds since the Unix epoch.
    pub started_at: u64,
    pub duration_secs: u64,
    /// For example "1080p60".
    pub video: String,
    pub frames_lost: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_uses_preset_or_override() {
        let mut s = StreamSettings {
            resolution: VideoResolution::P1080,
            fps: 60,
            codec: VideoCodec::H265,
            ..Default::default()
        };
        let preset = s.video_profile();
        assert_eq!(
            (preset.width, preset.height, preset.max_fps),
            (1920, 1080, 60)
        );
        assert_eq!(preset.codec, Codec::H265);

        s.bitrate_kbps = Some(30_000);
        assert_eq!(s.video_profile().bitrate, 30_000);
        s.bitrate_kbps = Some(1);
        assert_eq!(s.video_profile().bitrate, BITRATE_RANGE_KBPS.0);
        s.fps = 45;
        assert_eq!(s.video_profile().max_fps, 30);
    }

    #[test]
    fn toml_names() {
        let s: StreamSettings =
            toml::from_str("resolution = \"1080p\"\ncodec = \"h265\"\ndecoder = \"software\"")
                .unwrap();
        assert_eq!(s.resolution, VideoResolution::P1080);
        assert_eq!(s.codec, VideoCodec::H265);
        assert_eq!(s.decoder, DecoderPreference::Software);
        assert_eq!(s.fps, 60);
    }
}
