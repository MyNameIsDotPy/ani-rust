mod api;
mod app;
mod catalog;
mod cli;
mod downloader;
mod filenames;
mod hls;
mod models;
mod subtitles;
mod tui;
use anyhow::{Context, Result};
use clap::Parser;
use futures_util::{stream, StreamExt};
#[tokio::main]
async fn main() -> Result<()> {
    let mut args = cli::Cli::parse();
    let interactive = tui::enabled(args.plain, args.debug);
    let unified = interactive && matches!(args.command, None | Some(cli::Command::Search { .. }));
    tracing_subscriber::fmt()
        .with_env_filter(if args.debug {
            "ani_rust=debug"
        } else if unified {
            "off"
        } else if interactive {
            "ani_rust=error"
        } else {
            "ani_rust=info"
        })
        .with_writer(std::io::stderr)
        .init();
    if unified {
        let initial = match args.command.take() {
            Some(cli::Command::Search {
                query,
                limit,
                include_adult,
            }) => Some((query, limit, include_adult)),
            _ => None,
        };
        return app::run(&args, initial).await;
    }
    let http = api::Http::new(args.retries, args.rate_limit)?;
    let resolver = api::ApiResolver {
        http: http.clone(),
        endpoint: args.api_url.parse()?,
    };
    let run = async {
        match args
            .command
            .context("abre ani-rust en una terminal interactiva o indica un comando; usa --help")?
        {
            cli::Command::Search {
                query,
                limit,
                include_adult,
            } => {
                let client = catalog::AnimeXClient::new(&args.graphql_url, args.retries)?;
                let results = client
                    .search_anime(&query, limit as usize, include_adult)
                    .await?;
                catalog::display(&results);
                Ok(())
            }
            cli::Command::Inspect { source, episode } => {
                let saved = match &source.source_json {
                    Some(path) => Some(api::SavedResolver::load(path, episode).await?),
                    None => None,
                };
                let resolver: &dyn api::SourceResolver = saved
                    .as_ref()
                    .map(|s| s as &dyn api::SourceResolver)
                    .unwrap_or(&resolver);
                downloader::inspect(&http, resolver, &source, episode).await
            }
            cli::Command::Download {
                source,
                episodes,
                output,
                concurrency,
                ffmpeg,
            } => {
                let episodes = cli::episodes(&episodes).map_err(anyhow::Error::msg)?;
                anyhow::ensure!(source.source_json.is_none() || episodes.len() == 1,
                    "--source-json requires exactly one episode; each episode needs its own resolver response");
                let saved = match &source.source_json {
                    Some(path) => Some(api::SavedResolver::load(path, episodes[0]).await?),
                    None => None,
                };
                let resolver: &dyn api::SourceResolver = saved
                    .as_ref()
                    .map(|s| s as &dyn api::SourceResolver)
                    .unwrap_or(&resolver);
                tokio::fs::create_dir_all(&output).await?;
                let output = tokio::fs::canonicalize(output).await?;
                let bars = indicatif::MultiProgress::new();
                if interactive {
                    bars.set_draw_target(indicatif::ProgressDrawTarget::hidden());
                }
                let progress: Vec<_> = episodes
                    .iter()
                    .map(|number| {
                        let pb = bars.add(indicatif::ProgressBar::new(0));
                        pb.set_style(
                            indicatif::ProgressStyle::with_template(
                                "{spinner} {msg} [{bar:30}] {pos}/{len}",
                            )
                            .unwrap(),
                        );
                        pb.set_message(format!("episode {number}: queued"));
                        pb
                    })
                    .collect();
                let jobs = stream::iter(episodes.into_iter().zip(progress.iter().cloned()))
                    .map(|(number, pb)| {
                        let (http, resolver, source, output, ffmpeg) =
                            (&http, resolver, &source, &output, &ffmpeg);
                        async move {
                            pb.set_message(format!("episode {number}: downloading"));
                            if !interactive {
                                tracing::debug!(episode = number, "starting episode");
                            }
                            let result = downloader::episode(
                                http,
                                resolver,
                                source,
                                number,
                                output,
                                ffmpeg,
                                pb.clone(),
                            )
                            .await;
                            if result.is_err() {
                                pb.abandon_with_message(format!("episode {number}: failed"));
                            }
                            (number, result)
                        }
                    })
                    .buffer_unordered(concurrency as usize);
                let collect = jobs.collect::<Vec<_>>();
                tokio::pin!(collect);
                let mut screen = if interactive {
                    Some(tui::Screen::new()?)
                } else {
                    None
                };
                let mut tick = tokio::time::interval(std::time::Duration::from_millis(100));
                let results = loop {
                    tokio::select! {
                        results = &mut collect => break results,
                        _ = tick.tick(), if interactive => {
                            if let Some(s) = &mut screen { s.downloads(&progress)?; }
                            anyhow::ensure!(!tui::cancel_pressed()?, "downloads cancelled");
                        }
                    }
                };
                drop(screen);
                for (number, result) in &results {
                    if let Err(e) = result {
                        tracing::error!(episode=number,error=%format!("{e:#}"),"download failed");
                    }
                }
                let failures = results.iter().filter(|(_, r)| r.is_err()).count();
                println!(
                    "{} episode(s) completed or skipped; {failures} failed",
                    results.len() - failures
                );
                anyhow::ensure!(failures == 0, "{failures} episode(s) failed");
                Ok(())
            }
        }
    };
    tokio::select! { result = run => result, _ = tokio::signal::ctrl_c() => { anyhow::bail!("interrupted; unfinished staging data removed") } }
}
