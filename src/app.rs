//! A single terminal session owns search, configuration, inspection and downloads.
use crate::{
    api::{ApiResolver, Http, SavedResolver, SourceResolver},
    catalog::{AnimeSearchResult, AnimeXClient},
    cli::{self, Cli, Language, SourceArgs},
    downloader,
    tui::Screen,
};
use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use futures_util::{stream, StreamExt};
use ratatui::{
    layout::{Constraint, Layout},
    style::{Color, Style},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};
use std::{future::Future, path::PathBuf, time::Duration};

#[derive(Clone, Copy, PartialEq, Debug)]
enum Page {
    Search,
    Results,
    Configure,
    Paste,
    Report,
}
struct App {
    page: Page,
    query: String,
    results: Vec<AnimeSearchResult>,
    selection: ListState,
    fields: Vec<String>,
    focus: usize,
    notice: String,
    pasted: String,
    paste_backup: String,
    report: String,
    scroll: u16,
    limit: usize,
    adult: bool,
}
const LABELS: [&str; 9] = [
    "Episodios (1,3,5-10)",
    "Idioma (sub/dub)",
    "Proveedor",
    "Calidad (best/720p)",
    "Carpeta de salida",
    "Descargas simultáneas (1-64)",
    "Programa ffmpeg",
    "Archivo JSON (opcional)",
    "Formato (Espacio: alternar)",
];
impl App {
    fn new() -> Self {
        Self {
            page: Page::Search,
            query: String::new(),
            results: Vec::new(),
            selection: ListState::default(),
            fields: vec![
                "1".into(),
                "sub".into(),
                "beep".into(),
                "best".into(),
                "./downloads".into(),
                "3".into(),
                "ffmpeg".into(),
                String::new(),
                "video".into(),
            ],
            focus: 0,
            notice: String::new(),
            pasted: String::new(),
            paste_backup: String::new(),
            report: String::new(),
            scroll: 0,
            limit: 10,
            adult: false,
        }
    }
    fn selected(&self) -> Result<&AnimeSearchResult> {
        self.selection
            .selected()
            .and_then(|i| self.results.get(i))
            .context("Selecciona un anime")
    }
    fn plan(&self) -> Result<Plan> {
        let episodes = cli::episodes(self.fields[0].trim()).map_err(anyhow::Error::msg)?;
        let language = match self.fields[1].trim() {
            "sub" => Language::Sub,
            "dub" => Language::Dub,
            _ => anyhow::bail!("El idioma debe ser sub o dub"),
        };
        let quality = self.fields[3].trim();
        anyhow::ensure!(
            quality == "best"
                || quality
                    .strip_suffix('p')
                    .unwrap_or(quality)
                    .parse::<u32>()
                    .is_ok_and(|n| n > 0),
            "Calidad inválida: usa best o 720p"
        );
        let concurrency: usize = self.fields[5]
            .trim()
            .parse()
            .context("Concurrencia inválida")?;
        anyhow::ensure!(
            (1..=64).contains(&concurrency),
            "Concurrencia permitida: 1-64"
        );
        anyhow::ensure!(
            !self.fields[2].trim().is_empty()
                && !self.fields[4].trim().is_empty()
                && (self.fields[8] == "m3u8" || !self.fields[6].trim().is_empty()),
            "Proveedor, carpeta y ffmpeg son obligatorios"
        );
        let source_json = (!self.fields[7].trim().is_empty())
            .then(|| PathBuf::from(self.fields[7].trim().trim_matches('"')));
        anyhow::ensure!(
            (source_json.is_none() && self.pasted.trim().is_empty()) || episodes.len() == 1,
            "Una respuesta JSON corresponde a un solo episodio; indica un único número"
        );
        Ok(Plan {
            source: SourceArgs {
                anime_id: self.selected()?.id.clone(),
                playlist_only: self.fields[8] == "m3u8",
                source_json,
                language,
                provider: self.fields[2].trim().into(),
                quality: quality.into(),
                display_name: self
                    .selected()
                    .ok()
                    .and_then(|r| r.title_english.clone().or_else(|| r.title_romaji.clone())),
            },
            episodes,
            output: self.fields[4].trim().trim_matches('"').into(),
            concurrency,
            ffmpeg: self.fields[6].trim().trim_matches('"').into(),
            pasted: self.pasted.clone(),
        })
    }
    fn show_report(&mut self, report: String) {
        self.report = report;
        self.scroll = 0;
        self.page = Page::Report;
    }
    fn draw(&mut self, f: &mut Frame) {
        let rows = Layout::vertical([
            Constraint::Length(3),
            Constraint::Min(3),
            Constraint::Length(5),
        ])
        .split(f.area());
        f.render_widget(
            Paragraph::new("ani-rust  /  Buscar → Seleccionar → Descargar")
                .style(Style::default().fg(Color::Cyan))
                .block(Block::default().borders(Borders::ALL)),
            rows[0],
        );
        let help = match self.page {
            Page::Search => {
                f.render_widget(Paragraph::new(format!("Título: {}\n\nResultados: {}   Incluir adultos: {}\n\nEscribe un título y pulsa Enter.\nF2 cambia el filtro de adultos.\nF3 alterna entre 10, 25 y 50 resultados.",clean(&self.query),self.limit,if self.adult { "sí" } else { "no" })).block(Block::default().title(" Buscar anime ").borders(Borders::ALL)).wrap(Wrap{trim:false}),rows[1]);
                "Enter: buscar · Esc: salir · Ctrl-U: borrar · F2: adultos · F3: límite"
            }
            Page::Results => {
                let cols =
                    Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)])
                        .split(rows[1]);
                let items: Vec<_> = self
                    .results
                    .iter()
                    .map(|r| {
                        ListItem::new(clean(
                            r.title_english
                                .as_deref()
                                .or(r.title_romaji.as_deref())
                                .unwrap_or(&r.id),
                        ))
                    })
                    .collect();
                f.render_stateful_widget(
                    List::new(items)
                        .highlight_symbol("› ")
                        .highlight_style(Style::default().bg(Color::Cyan).fg(Color::Black))
                        .block(Block::default().title(" Resultados ").borders(Borders::ALL)),
                    cols[0],
                    &mut self.selection,
                );
                let text=self.selected().map(|r|format!("{}\n\nID: {}\nEpisodios: {}\nAño: {}\nFormato: {}\nEstado: {}\n\nGéneros: {}",r.title_english.as_deref().or(r.title_romaji.as_deref()).unwrap_or(&r.id),r.id,r.episode_count.map(|n|n.to_string()).unwrap_or("?".into()),r.season_year.map(|n|n.to_string()).unwrap_or("?".into()),r.format.as_deref().unwrap_or("?"),r.status.as_deref().unwrap_or("?"),r.genres.as_ref().map(|g|g.join(", ")).unwrap_or_default())).unwrap_or("Sin resultados. Esc para otra búsqueda.".into());
                f.render_widget(
                    Paragraph::new(clean_lines(&text))
                        .wrap(Wrap { trim: false })
                        .block(Block::default().title(" Detalles ").borders(Borders::ALL)),
                    cols[1],
                );
                "↑/↓: seleccionar · Enter: configurar descarga · Esc: otra búsqueda"
            }
            Page::Configure => {
                let title = self.selected().map(|r| clean(&r.id)).unwrap_or_default();
                let items: Vec<_> = LABELS
                    .iter()
                    .enumerate()
                    .map(|(i, label)| {
                        ListItem::new(format!(
                            "{label}: {}",
                            if i == 7 && !self.pasted.is_empty() {
                                "[JSON pegado]".into()
                            } else {
                                clean(&self.fields[i])
                            }
                        ))
                    })
                    .collect();
                let mut state = ListState::default().with_selected(Some(self.focus));
                f.render_stateful_widget(
                    List::new(items)
                        .highlight_symbol("› ")
                        .highlight_style(Style::default().bg(Color::Cyan).fg(Color::Black))
                        .block(
                            Block::default()
                                .title(format!(" Descargar {title} "))
                                .borders(Borders::ALL),
                        ),
                    rows[1],
                    &mut state,
                );
                "Tab/↑/↓: campo · Ctrl-U: borrar · F2: pegar JSON · F4: inspeccionar\nF5: descargar · Esc: volver a resultados"
            }
            Page::Paste => {
                let lines = self.pasted.lines().count();
                let offset = lines
                    .saturating_sub(rows[1].height.saturating_sub(2) as usize)
                    .min(u16::MAX as usize) as u16;
                f.render_widget(
                    Paragraph::new(clean_lines(&self.pasted))
                        .scroll((offset, 0))
                        .wrap(Wrap { trim: false })
                        .block(
                            Block::default()
                                .title(" Pega aquí la respuesta JSON autorizada ")
                                .borders(Borders::ALL),
                        ),
                    rows[1],
                );
                "Pega JSON desde el portapapeles · F2: aplicar · Ctrl-U: borrar · Esc: volver"
            }
            Page::Report => {
                f.render_widget(
                    Paragraph::new(clean_lines(&self.report))
                        .scroll((self.scroll, 0))
                        .wrap(Wrap { trim: false })
                        .block(Block::default().title(" Resultado ").borders(Borders::ALL)),
                    rows[1],
                );
                "↑/↓ o RePág/AvPág: desplazar · Enter/Esc: configuración · /: otra búsqueda"
            }
        };
        f.render_widget(
            Paragraph::new(format!("{help}\n{}", clean(&self.notice)))
                .wrap(Wrap { trim: false })
                .block(Block::default().borders(Borders::ALL)),
            rows[2],
        );
    }
}
struct Plan {
    source: SourceArgs,
    episodes: Vec<u32>,
    output: PathBuf,
    concurrency: usize,
    ffmpeg: PathBuf,
    pasted: String,
}
impl Plan {
    async fn saved(&self) -> Result<Option<SavedResolver>> {
        if !self.pasted.trim().is_empty() {
            Ok(Some(SavedResolver::from_json(
                self.pasted.as_bytes(),
                self.episodes[0],
            )?))
        } else if let Some(path) = &self.source.source_json {
            Ok(Some(SavedResolver::load(path, self.episodes[0]).await?))
        } else {
            Ok(None)
        }
    }
}
fn clean(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}
fn clean_lines(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .collect()
}
fn edit(text: &mut String, key: KeyCode, modifiers: KeyModifiers) {
    match key {
        KeyCode::Backspace => {
            text.pop();
        }
        KeyCode::Char('u') if modifiers.contains(KeyModifiers::CONTROL) => text.clear(),
        KeyCode::Char(c) if !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
            text.push(c)
        }
        _ => {}
    }
}
async fn wait<T>(
    screen: &mut Screen,
    label: &str,
    bars: &[indicatif::ProgressBar],
    future: impl Future<Output = Result<T>>,
) -> Result<Option<T>> {
    tokio::pin!(future);
    let mut tick = tokio::time::interval(Duration::from_millis(80));
    loop {
        tokio::select! {
            result=&mut future=>return result.map(Some),
            _=tick.tick()=>{
                if bars.is_empty() { screen.terminal.draw(|f|{f.render_widget(Paragraph::new(format!("{label}\n\nProcesando…\nEsc: cancelar y volver")).block(Block::default().borders(Borders::ALL)),f.area());})?; }
                else {screen.downloads(bars)?;}
                if event::poll(Duration::ZERO)? {if let Event::Key(k)=event::read()? {if k.kind==KeyEventKind::Press && (k.code==KeyCode::Esc || (!bars.is_empty() && k.code==KeyCode::Char('q')) || (k.code==KeyCode::Char('c')&&k.modifiers.contains(KeyModifiers::CONTROL))) {return Ok(None);}}}
            }
        }
    }
}
async fn search(app: &mut App, screen: &mut Screen, catalog: &AnimeXClient) -> Result<()> {
    let result = wait(
        screen,
        "Buscando en AnimeX",
        &[],
        catalog.search_anime(&app.query, app.limit, app.adult),
    )
    .await;
    match result {
        Ok(Some(items)) => {
            app.results = items;
            app.selection.select((!app.results.is_empty()).then_some(0));
            app.page = Page::Results;
            app.notice.clear();
        }
        Ok(None) => app.notice = "Búsqueda cancelada".into(),
        Err(e) => app.notice = format!("{e:#}"),
    }
    Ok(())
}
async fn execute(
    plan: &Plan,
    http: &Http,
    api: &ApiResolver,
    bars: &[indicatif::ProgressBar],
    inspect: bool,
) -> Result<String> {
    let saved = plan.saved().await?;
    let resolver: &dyn SourceResolver = saved
        .as_ref()
        .map(|s| s as &dyn SourceResolver)
        .unwrap_or(api);
    if inspect {
        return downloader::inspection(http, resolver, &plan.source, plan.episodes[0]).await;
    }
    tokio::fs::create_dir_all(&plan.output).await?;
    let output = tokio::fs::canonicalize(&plan.output).await?;
    let results = stream::iter(plan.episodes.iter().copied().zip(bars.iter().cloned()))
        .map(|(number, pb)| {
            let output = &output;
            async move {
                pb.set_message(format!("Episodio {number}: descargando"));
                let result = downloader::episode(
                    http,
                    resolver,
                    &plan.source,
                    number,
                    output,
                    &plan.ffmpeg,
                    pb.clone(),
                )
                .await;
                match result {
                    Ok(()) => format!("Episodio {number}: completo u omitido"),
                    Err(e) => {
                        pb.abandon_with_message(format!("Episodio {number}: error"));
                        format!("Episodio {number}: {e:#}")
                    }
                }
            }
        })
        .buffer_unordered(plan.concurrency)
        .collect::<Vec<_>>()
        .await;
    let mut report = format!("Carpeta: {}\n\n{}", output.display(), results.join("\n\n"));
    if report.contains("403") {
        report.push_str("\n\nEl proveedor denegó el acceso. Vuelve con Enter y usa F2 para pegar una respuesta JSON autorizada, o indica su archivo. Se admite un episodio por respuesta.");
    }
    Ok(report)
}
pub async fn run(args: &Cli, initial: Option<(String, u32, bool)>) -> Result<()> {
    let http = Http::new(args.retries, args.rate_limit)?;
    let api = ApiResolver {
        http: http.clone(),
        endpoint: args.api_url.parse()?,
    };
    let catalog = AnimeXClient::new(&args.graphql_url, args.retries)?;
    let mut app = App::new();
    let mut screen = Screen::new()?;
    if let Some((query, limit, adult)) = initial {
        app.query = query;
        app.limit = limit as usize;
        app.adult = adult;
        search(&mut app, &mut screen, &catalog).await?;
    }
    loop {
        screen.terminal.draw(|f| app.draw(f))?;
        if !event::poll(Duration::ZERO)? {
            tokio::time::sleep(Duration::from_millis(40)).await;
            continue;
        }
        let ev = event::read()?;
        if let Event::Paste(text) = ev {
            match app.page {
                Page::Search => app.query.push_str(&clean(&text)),
                Page::Configure => {
                    if app.focus == 7 {
                        app.pasted.clear();
                    }
                    app.fields[app.focus].push_str(&clean(&text));
                }
                Page::Paste => app.pasted.push_str(&text),
                _ => {}
            }
            continue;
        }
        let Event::Key(k) = ev else {
            continue;
        };
        if k.kind != KeyEventKind::Press {
            continue;
        }
        if k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL) {
            return Ok(());
        }
        match app.page {
            Page::Search => match k.code {
                KeyCode::Esc => return Ok(()),
                KeyCode::F(2) => app.adult = !app.adult,
                KeyCode::F(3) => {
                    app.limit = match app.limit {
                        10 => 25,
                        25 => 50,
                        _ => 10,
                    }
                }
                KeyCode::Enter => search(&mut app, &mut screen, &catalog).await?,
                _ => edit(&mut app.query, k.code, k.modifiers),
            },
            Page::Results => match k.code {
                KeyCode::Esc => {
                    app.page = Page::Search;
                    app.notice.clear();
                }
                KeyCode::Down if !app.results.is_empty() => app.selection.select(Some(
                    (app.selection.selected().unwrap_or(0) + 1).min(app.results.len() - 1),
                )),
                KeyCode::Up => app
                    .selection
                    .select(app.selection.selected().map(|i| i.saturating_sub(1))),
                KeyCode::Enter if app.selected().is_ok() => {
                    app.page = Page::Configure;
                    app.pasted.clear();
                    app.fields[7].clear();
                    app.notice.clear();
                }
                _ => {}
            },
            Page::Configure => match k.code {
                KeyCode::Char(' ') | KeyCode::Enter if app.focus == 8 => {
                    app.fields[8] = if app.fields[8] == "video" {
                        "m3u8"
                    } else {
                        "video"
                    }
                    .into();
                }
                KeyCode::Esc => app.page = Page::Results,
                KeyCode::Tab | KeyCode::Down => app.focus = (app.focus + 1) % LABELS.len(),
                KeyCode::BackTab | KeyCode::Up => {
                    app.focus = (app.focus + LABELS.len() - 1) % LABELS.len()
                }
                KeyCode::F(2) => {
                    app.paste_backup = app.pasted.clone();
                    app.page = Page::Paste;
                    app.notice.clear();
                }
                KeyCode::F(4) | KeyCode::F(5) => match app.plan() {
                    Err(e) => app.notice = format!("{e:#}"),
                    Ok(plan) => {
                        let inspect = k.code == KeyCode::F(4);
                        let bars: Vec<_> = if inspect {
                            Vec::new()
                        } else {
                            plan.episodes
                                .iter()
                                .map(|n| {
                                    let b = indicatif::ProgressBar::hidden();
                                    b.set_message(format!("Episodio {n}: en espera"));
                                    b
                                })
                                .collect()
                        };
                        let result = wait(
                            &mut screen,
                            if inspect {
                                "Inspeccionando el primer episodio indicado"
                            } else {
                                "Descargando"
                            },
                            &bars,
                            execute(&plan, &http, &api, &bars, inspect),
                        )
                        .await;
                        app.show_report(match result {Ok(Some(s))=>s,Ok(None)=>"Operación cancelada. Los episodios ya publicados se conservan.\nPuedes ajustar la configuración y volver a intentar.".into(),Err(e)=>format!("{e:#}\n\nEnter: volver a la configuración. Si tienes una respuesta autorizada, pégala con F2.")});
                    }
                },
                _ => {
                    if app.focus == 8 {
                        continue;
                    }
                    if app.focus == 7 {
                        app.pasted.clear();
                    }
                    let focus = app.focus;
                    edit(&mut app.fields[focus], k.code, k.modifiers);
                }
            },
            Page::Paste => match k.code {
                KeyCode::Esc => {
                    app.pasted = app.paste_backup.clone();
                    app.page = Page::Configure;
                }
                KeyCode::F(2) => match SavedResolver::from_json(app.pasted.as_bytes(), 1) {
                    Ok(_) => {
                        app.fields[7].clear();
                        app.page = Page::Configure;
                        app.notice = "JSON listo. Indica el episodio correcto y pulsa F5.".into();
                    }
                    Err(e) => app.notice = format!("{e:#}"),
                },
                KeyCode::Enter => app.pasted.push('\n'),
                _ => edit(&mut app.pasted, k.code, k.modifiers),
            },
            Page::Report => match k.code {
                KeyCode::Enter | KeyCode::Esc => {
                    app.page = Page::Configure;
                    app.notice.clear();
                }
                KeyCode::Char('/') => app.page = Page::Search,
                KeyCode::Down => app.scroll = app.scroll.saturating_add(1),
                KeyCode::Up => app.scroll = app.scroll.saturating_sub(1),
                KeyCode::PageDown => app.scroll = app.scroll.saturating_add(10),
                KeyCode::PageUp => app.scroll = app.scroll.saturating_sub(10),
                _ => {}
            },
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn selected_app() -> App {
        let mut a = App::new();
        a.results =
            vec![
                serde_json::from_str(r#"{"id":"internal-fixture","titleEnglish":"Fixture"}"#)
                    .unwrap(),
            ];
        a.selection.select(Some(0));
        a
    }
    #[test]
    fn plan_uses_internal_id_and_rejects_shared_json() {
        let mut a = selected_app();
        a.fields[0] = "1-3".into();
        assert_eq!(a.plan().unwrap().source.anime_id, "internal-fixture");
        a.pasted = "{}".into();
        assert!(a.plan().is_err());
        a.fields[0] = "2".into();
        assert!(a.plan().is_ok());
        a.fields[5] = "0".into();
        assert!(a.plan().is_err());
    }
    #[test]
    fn renders_every_page_and_small_terminal() {
        for (width, height) in [(100, 30), (40, 12)] {
            for page in [
                Page::Search,
                Page::Results,
                Page::Configure,
                Page::Paste,
                Page::Report,
            ] {
                let mut a = selected_app();
                a.page = page;
                let mut t =
                    ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height))
                        .unwrap();
                t.draw(|f| a.draw(f)).unwrap();
                let text = t
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .map(|c| c.symbol())
                    .collect::<String>();
                assert!(text.contains("ani-rust"));
            }
        }
    }
}
