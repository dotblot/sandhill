//! Calls that start sessions and IRkernel, redraw plots, preview documents and serve pages.

use super::*;

impl Daemon {
    pub(super) async fn list_r_without_irkernel(&self, project: Option<String>) -> Result<Value> {
        let root = project.map(|p| projects::normalize(&p)).transpose()?;
        let own = self.own_folder();
        let found = blocking(move || {
            let search = crate::kernelspecs::Search::from_env()
                .in_project(root.as_deref().map(Path::new))
                .in_own(own);
            Ok(crate::irkernel::find(&search)
                .iter()
                .map(|candidate| candidate.info(&search))
                .collect::<Vec<_>>())
        })
        .await?;
        Ok(serde_json::to_value(found)?)
    }

    pub(super) async fn install_irkernel(
        &self,
        r: String,
        project: Option<String>,
        library: Option<String>,
    ) -> Result<Value> {
        let project = project.map(|p| projects::normalize(&p)).transpose()?;
        let root = project.clone();
        let record = self.install_record();
        let executable = self.options.executable.clone();
        let own = self.own_folder();
        let (mut launch, trust) = blocking(move || {
            let search = crate::kernelspecs::Search::from_env()
                .in_project(root.as_deref().map(Path::new))
                .in_own(own);
            crate::irkernel::launch(&executable, &r, library.as_deref(), &search, &record)
        })
        .await?;
        // pixi reads the project's manifest; plain R and conda don't
        // act on the project's files.
        if let Some(folder) = trust {
            self.trusted(vec![folder], "install IRkernel with pixi")
                .await?;
        }
        // Listed with the project's jobs.
        launch.project = project;
        let started = self.terminals.start(launch).await?;
        self.keep_awake(&started.id);
        Ok(serde_json::to_value(started)?)
    }

    pub(super) async fn start_session(
        &self,
        kernel: String,
        cwd: Option<String>,
        project: Option<String>,
    ) -> Result<Value> {
        let cwd = match cwd {
            Some(cwd) => PathBuf::from(cwd),
            None => std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/")),
        };
        if !cwd.is_dir() {
            anyhow::bail!("{} is not a directory", cwd.display());
        }
        let project = project.map(|p| projects::normalize(&p)).transpose()?;
        let env = match project.clone() {
            Some(root) => blocking(move || Ok(projects::kernel_env(Path::new(&root)))).await?,
            None => Vec::new(),
        };
        let _guard = self.own_sessions.read().await;
        let handle = SessionHandle::start(
            self.journal.clone(),
            &self.state.kernels_dir(),
            &kernel,
            &cwd,
            session::Start { project, env },
        )
        .await?;
        let id = handle.id.clone();
        self.insert(handle);
        Ok(serde_json::to_value(self.session_info(&id)?)?)
    }

    pub(super) async fn preview_document(
        &self,
        path: String,
        project: Option<String>,
    ) -> Result<Value> {
        let file = files::resolve(&path)?;
        if !crate::preview::previewable(&file) {
            bail!("{} is not a Quarto document (.qmd or .Rmd)", file.display());
        }
        if !file.is_file() {
            bail!("{} does not exist", file.display());
        }
        let project = project.map(|p| projects::normalize(&p)).transpose()?;
        let (id, port) = match crate::preview::running(&self.terminals, &file) {
            Some(found) => found,
            None => {
                let own = self.own_folder();
                let quarto = blocking(move || {
                    Ok(crate::preview::find_quarto(
                        &crate::kernelspecs::Search::from_env().in_own(own),
                    ))
                })
                .await?
                .with_context(crate::preview::install_advice)?;
                let port = crate::preview::free_port()?;
                let mut launch = crate::preview::launch(&quarto, &file, port, project.clone());
                if let Some(root) = project {
                    launch.env =
                        blocking(move || Ok(crate::preview::env(Path::new(&root)))).await?;
                }
                (self.terminals.start(launch).await?.id, port)
            }
        };
        Ok(serde_json::to_value(
            crate::preview::ready(&self.terminals, &id, port).await?,
        )?)
    }

    pub(super) async fn serve_html(
        &self,
        html: Option<String>,
        blob: Option<String>,
    ) -> Result<Value> {
        let html = match (html, blob) {
            (Some(html), _) => html,
            (None, Some(hash)) => {
                let bytes = self
                    .journal
                    .blobs()
                    .get(&hash)
                    .with_context(|| format!("no blob {hash}"))?;
                String::from_utf8_lossy(&bytes).into_owned()
            }
            (None, None) => bail!("give html or a blob"),
        };
        Ok(serde_json::to_value(self.pages.serve_html(&html).await?)?)
    }

    /// `RedrawPlot` with `show`: the plot comes back as a PNG.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn redraw_plot_shown(
        &self,
        session: String,
        run: String,
        image: Option<String>,
        index: u32,
        width: f64,
        height: f64,
        dpi: u32,
    ) -> Result<Value> {
        // Drawn to a file of the daemon's, then sent back and removed.
        let dir = self.state.root().join("redraw");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{}.png", uuid::Uuid::new_v4().simple()));
        let file = path.to_string_lossy().into_owned();
        let save = redraw::Save {
            file: &file,
            format: "png",
            width,
            height,
            dpi,
            rasterize: false,
        };
        let drawn = self
            .redraw_plot(&session, &run, image.as_deref(), index, Some(save))
            .await;
        let bytes = std::fs::read(&path);
        let _ = std::fs::remove_file(&path);
        let mut drawn = drawn?;
        if drawn.can {
            drawn.png = Some(BASE64.encode(bytes?));
        }
        Ok(serde_json::to_value(drawn)?)
    }

    /// `RedrawPlot` without `show`: drawn into `file` if one is given.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn redraw_plot_saved(
        &self,
        session: String,
        run: String,
        image: Option<String>,
        index: u32,
        file: Option<String>,
        format: String,
        width: f64,
        height: f64,
        dpi: u32,
        rasterize: bool,
    ) -> Result<Value> {
        let save = file.as_deref().map(|file| redraw::Save {
            file,
            format: &format,
            width,
            height,
            dpi,
            rasterize,
        });
        Ok(serde_json::to_value(
            self.redraw_plot(&session, &run, image.as_deref(), index, save)
                .await?,
        )?)
    }
}
