// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use clap::{Parser, Subcommand};
use settings::Binding;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "settingsd",
    version,
    about = "Native ABP desktop settings authority"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(clap::Args)]
struct Profile {
    #[arg(long)]
    root: Option<PathBuf>,
    #[arg(long)]
    instance: String,
    #[arg(long, default_value = "default")]
    profile: String,
}
impl Profile {
    fn resolve(self) -> anyhow::Result<(PathBuf, Binding)> {
        let binding = Binding {
            instance: self.instance,
            profile: self.profile,
        };
        binding.validate().map_err(|e| anyhow::anyhow!(e.message))?;
        let root = self.root.unwrap_or_else(|| {
            config::path(config::Dir::Etc)
                .join("settings")
                .join(&binding.profile)
        });
        Ok((root, binding))
    }
}
#[derive(Subcommand)]
enum Command {
    /// Explicitly create a new profile. Never overwrites existing state.
    Init(Profile),
    /// Validate session state; creation requires explicit installer permission.
    Seed {
        #[command(flatten)]
        profile: Profile,
        /// Installer-only first provisioning; never used by automatic startup.
        #[arg(long)]
        allow_create: bool,
    },
    /// Serve an established profile; missing/unsupported state fails visibly.
    /// The machine's authority by default; `--mirror-of NODE` serves a
    /// desk's mirror of that node's authority from its sealed cache.
    Serve {
        #[command(flatten)]
        profile: Profile,
        /// Serve the profile store (the default).
        #[arg(long, conflicts_with = "mirror_of")]
        authority: bool,
        #[arg(long, value_name = "NODE")]
        mirror_of: Option<String>,
    },
    /// Migrate a format-4 profile to format 5 (it also migrates on open).
    /// `--dry-run` prints the result and the receipt and writes nothing.
    Migrate {
        #[command(flatten)]
        profile: Profile,
        #[arg(long)]
        dry_run: bool,
        /// Import backdrop's document into `wallpaper.*` once, then rename
        /// it `PATH.migrated`. Beside the authority only, settingsd stopped.
        #[arg(long, value_name = "PATH")]
        import_backdrop: Option<PathBuf>,
    },
    /// Write the profile back as format 4 for the previous settingsd
    /// (`--target`), proved with its own `seed`. Run with settingsd stopped.
    Downgrade {
        #[command(flatten)]
        profile: Profile,
        #[arg(long)]
        to: u32,
        /// The previous settingsd binary.
        #[arg(long)]
        target: PathBuf,
        /// Take the kept format-4 record, losing every change since the
        /// migration, instead of the reverse conversion.
        #[arg(long)]
        from_kept: bool,
    },
    /// Set aside a profile this node no longer serves, as
    /// `*.retired-<instance>`. Run with settingsd stopped.
    Retire(Profile),
    /// Move an established profile to a renamed instance (after a machine
    /// rename). Run with settingsd stopped; both stored documents must verify
    /// under --from-instance and are re-sealed under --instance.
    Rebind {
        #[command(flatten)]
        profile: Profile,
        #[arg(long)]
        from_instance: String,
    },
}
fn main() -> anyhow::Result<()> {
    buildinfo::exit_on_version!();
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .try_init();
    match Cli::parse().command {
        Command::Init(profile) => {
            let (root, binding) = profile.resolve()?;
            let (_store, accepted) =
                settingsd::store::Store::create(&root, binding, Default::default())?;
            println!(
                "{}",
                serde_json::json!({"status":"initialised","binding":accepted.binding,"incarnation":accepted.incarnation,"revision":accepted.revision})
            );
            Ok(())
        }
        Command::Seed {
            profile,
            allow_create,
        } => {
            let (root, binding) = profile.resolve()?;
            let (_store, accepted) = settingsd::store::Store::seed(&root, binding, allow_create)?;
            println!(
                "{}",
                serde_json::json!({"status":"seeded","binding":accepted.binding,"incarnation":accepted.incarnation,"revision":accepted.revision})
            );
            Ok(())
        }
        Command::Rebind {
            profile,
            from_instance,
        } => {
            let (root, binding) = profile.resolve()?;
            let from = Binding {
                instance: from_instance,
                profile: binding.profile.clone(),
            };
            from.validate().map_err(|e| anyhow::anyhow!(e.message))?;
            let (accepted, changed) = settingsd::store::Store::rebind(&root, &from, binding)?;
            println!(
                "{}",
                serde_json::json!({"status": if changed {"rebound"} else {"unchanged"},"from":from,"binding":accepted.binding,"incarnation":accepted.incarnation,"revision":accepted.revision,"format":accepted.format})
            );
            Ok(())
        }
        Command::Serve {
            profile,
            authority: _,
            mirror_of,
        } => {
            let (root, binding) = profile.resolve()?;
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?
                .block_on(settingsd::service::serve(root, binding, mirror_of))
        }
        Command::Migrate {
            profile,
            dry_run,
            import_backdrop,
        } => {
            let (root, binding) = profile.resolve()?;
            let result = if dry_run {
                settingsd::migration::dry_run(&root, &binding, import_backdrop.as_deref())?
            } else if let Some(path) = import_backdrop {
                settingsd::migration::import_backdrop(&root, &binding, &path)?
            } else {
                let (store, accepted) = settingsd::store::Store::open(&root, &binding)?;
                serde_json::json!({"status": if store.migrated {"migrated"} else {"current"},"binding":accepted.binding,"incarnation":accepted.incarnation,"revision":accepted.revision})
            };
            println!("{result}");
            if result["status"] == "refused" {
                std::process::exit(2);
            }
            Ok(())
        }
        Command::Downgrade {
            profile,
            to,
            target,
            from_kept,
        } => {
            let (root, binding) = profile.resolve()?;
            let result = settingsd::downgrade::downgrade(
                &root,
                &binding,
                to,
                from_kept,
                settingsd::downgrade::seed_with(&target),
            )?;
            println!("{result}");
            Ok(())
        }
        Command::Retire(profile) => {
            let (root, binding) = profile.resolve()?;
            let retired = settingsd::store::Store::retire(&root, &binding)?;
            println!(
                "{}",
                serde_json::json!({"status":"retired","binding":binding,"files":retired})
            );
            Ok(())
        }
    }
}
