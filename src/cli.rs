use clap::{Args, Parser, Subcommand, ValueEnum};
use std::{collections::BTreeSet, path::PathBuf};

#[derive(Parser)]
#[command(version, about = "Bulk download media you are authorized to save")]
pub struct Cli {
    #[arg(long, global = true)]
    pub debug: bool,
    /// Disable the interactive terminal UI (also automatic for pipes and --debug).
    #[arg(long, global = true)]
    pub plain: bool,
    #[arg(
        long,
        global = true,
        default_value = "https://pp.animex.one/rest/api/sources"
    )]
    pub api_url: String,
    #[arg(
        long,
        global = true,
        default_value = "https://graphql.animex.one/graphql"
    )]
    pub graphql_url: String,
    #[arg(long, global = true, default_value_t = 3, value_parser = clap::value_parser!(u32).range(0..=10))]
    pub retries: u32,
    /// Maximum HTTP requests per second sent to source/media servers, to avoid triggering anti-abuse blocks.
    #[arg(long, global = true, default_value_t = 5, value_parser = clap::value_parser!(u32).range(1..=50))]
    pub rate_limit: u32,
    #[command(subcommand)]
    pub command: Option<Command>,
}
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Language {
    Sub,
    Dub,
}
impl std::fmt::Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::Sub => "sub",
                Self::Dub => "dub",
            }
        )
    }
}
#[derive(Args, Clone)]
pub struct SourceArgs {
    pub anime_id: String,
    /// Save the selected HLS playlist and subtitles, without video segments.
    #[arg(long)]
    pub playlist_only: bool,
    /// Use a saved resolver response instead of the source API (one episode only).
    #[arg(long)]
    pub source_json: Option<PathBuf>,
    #[arg(long = "type", value_enum, default_value = "sub")]
    pub language: Language,
    #[arg(long, default_value = "beep")]
    pub provider: String,
    #[arg(long, default_value = "best", value_parser = quality)]
    pub quality: String,
    /// Human-readable title used for the parent download folder; falls back to anime_id when unset.
    #[arg(skip)]
    pub display_name: Option<String>,
}
fn quality(s: &str) -> Result<String, String> {
    if s == "best" || s.trim_end_matches('p').parse::<u32>().is_ok_and(|n| n > 0) {
        Ok(s.to_owned())
    } else {
        Err("expected best or a resolution such as 720p".into())
    }
}
#[derive(Subcommand)]
pub enum Command {
    Search {
        query: String,
        #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u32).range(1..=100))]
        limit: u32,
        #[arg(long, default_value_t = false, action = clap::ArgAction::Set)]
        include_adult: bool,
    },
    Inspect {
        #[command(flatten)]
        source: SourceArgs,
        #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..))]
        episode: u32,
    },
    Download {
        #[command(flatten)]
        source: SourceArgs,
        #[arg(long, required = true)]
        episodes: String,
        #[arg(long, default_value = "./downloads")]
        output: PathBuf,
        #[arg(long, default_value_t = 3, value_parser = clap::value_parser!(u32).range(1..=64))]
        concurrency: u32,
        #[arg(long, default_value = "ffmpeg")]
        ffmpeg: PathBuf,
    },
}
pub fn episodes(s: &str) -> Result<Vec<u32>, String> {
    let mut out = BTreeSet::new();
    for part in s.split(',') {
        let bounds: Vec<_> = part.trim().split('-').collect();
        let start = bounds[0].parse::<u32>().map_err(|_| "invalid episode")?;
        let end = match bounds.len() {
            1 => start,
            2 => bounds[1].parse::<u32>().map_err(|_| "invalid range")?,
            _ => return Err("invalid range".into()),
        };
        if start == 0 || end < start || end - start > 10000 {
            return Err(
                "episodes must be positive, ascending, and bounded to 10000 per range".into(),
            );
        }
        out.extend(start..=end);
        if out.len() > 10000 {
            return Err("maximum 10000 episodes per invocation".into());
        }
    }
    Ok(out.into_iter().collect())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ranges() {
        assert_eq!(episodes("1,3,5-7,3").unwrap(), vec![1, 3, 5, 6, 7]);
        for s in ["", "0", "4-2", "1-2-3", "1-99999999"] {
            assert!(episodes(s).is_err());
        }
    }
}
