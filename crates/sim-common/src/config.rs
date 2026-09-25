/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/

use anyhow::{anyhow, Result};
use std::{
    fmt::Display,
    path::{Path, PathBuf},
    str::FromStr,
};

pub const IMAGE_CONFIG_ROOT: &str = "/etc/notification-service";

pub const SCENARIO_RELATIVE_DEFAULT: &str = "dhall-configs/dev/sim.dhall";
pub const SERVICE_RELATIVE_DEFAULT: &str = "dhall-configs/dev/notification_service.dhall";

pub struct ConfigRoot {
    pub path: PathBuf,
    pub label: &'static str,
}

pub fn default_roots() -> Vec<ConfigRoot> {
    vec![
        ConfigRoot {
            path: PathBuf::from("."),
            label: "repo checkout, relative to the working directory",
        },
        ConfigRoot {
            path: PathBuf::from(IMAGE_CONFIG_ROOT),
            label: "container image, baked at build time",
        },
    ]
}

const SCENARIO_HINT: &str = "sim.dhall is self-contained; a scenario is the env vars set on top \
                             of it, not a file of its own.";
const SERVICE_HINT: &str = "This is the notification service's own config, read for its Redis \
                            settings and max_shards, not a scenario.";

pub fn sim_config_path() -> Result<String> {
    resolve(
        "SIM_CONFIG",
        SCENARIO_RELATIVE_DEFAULT,
        SCENARIO_HINT,
        &default_roots(),
    )
}

pub fn service_config_path() -> Result<String> {
    resolve(
        "DHALL_CONFIG",
        SERVICE_RELATIVE_DEFAULT,
        SERVICE_HINT,
        &default_roots(),
    )
}

pub fn client_index_start(scenario_value: u64) -> Result<u64> {
    env_override("CLIENT_INDEX_START", scenario_value)
}

pub fn env_override<T: FromStr>(key: &str, default: T) -> Result<T>
where
    T::Err: Display,
{
    match std::env::var(key) {
        Ok(raw) if raw.trim().is_empty() => Ok(default),
        Ok(raw) => raw
            .trim()
            .parse::<T>()
            .map_err(|err| anyhow!("invalid {key}: {err}")),
        Err(_) => Ok(default),
    }
}

pub fn env_override_list(key: &str, default: Vec<String>) -> Vec<String> {
    match std::env::var(key) {
        Ok(raw) => {
            let items: Vec<String> = raw
                .split(',')
                .map(|item| item.trim().to_string())
                .filter(|item| !item.is_empty())
                .collect();
            if items.is_empty() {
                default
            } else {
                items
            }
        }
        Err(_) => default,
    }
}

fn resolve(var: &str, relative_default: &str, hint: &str, roots: &[ConfigRoot]) -> Result<String> {
    resolve_with(var, std::env::var(var).ok(), relative_default, hint, roots)
}

fn resolve_with(
    var: &str,
    value: Option<String>,
    relative_default: &str,
    hint: &str,
    roots: &[ConfigRoot],
) -> Result<String> {
    match value {
        Some(raw) if raw.trim().is_empty() => Err(anyhow!(
            "{var} is set but empty. Unset it to fall back to {relative_default}, or point it at a config file."
        )),
        Some(raw) => {
            let path = raw.trim();
            if Path::new(path).is_file() {
                Ok(path.to_string())
            } else {
                Err(anyhow!(
                    "{var}={path} does not exist (working directory {}). In a pod this is usually a ConfigMap that is not mounted, or mounted somewhere else.",
                    working_directory()
                ))
            }
        }
        None => first_existing(roots, relative_default).ok_or_else(|| {
            anyhow!(
                "{var} is unset and no built-in default resolved (working directory {}).\n{}\n\
                 Set {var} explicitly: {relative_default} relative to a repository checkout, or \
                 the path it is mounted at in a pod. {hint}",
                working_directory(),
                describe_candidates(roots, relative_default)
            )
        }),
    }
}

fn first_existing(roots: &[ConfigRoot], relative: &str) -> Option<String> {
    roots
        .iter()
        .map(|root| root.path.join(relative))
        .find(|candidate| candidate.is_file())
        .map(|candidate| candidate.to_string_lossy().into_owned())
}

fn describe_candidates(roots: &[ConfigRoot], relative: &str) -> String {
    roots
        .iter()
        .map(|root| {
            let reason = if root.path.is_dir() {
                "directory exists, file absent"
            } else {
                "directory absent"
            };
            format!(
                "  tried {} ({}: {})",
                root.path.join(relative).display(),
                root.label,
                reason
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn working_directory() -> String {
    std::env::current_dir()
        .map(|dir| dir.display().to_string())
        .unwrap_or_else(|_| "<unknown>".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn roots(paths: &[&str]) -> Vec<ConfigRoot> {
        paths
            .iter()
            .map(|path| ConfigRoot {
                path: PathBuf::from(path),
                label: "test root",
            })
            .collect()
    }

    fn repo_and_missing_roots() -> Vec<ConfigRoot> {
        vec![
            ConfigRoot {
                path: PathBuf::from("/nonexistent-sim-root"),
                label: "absent root",
            },
            ConfigRoot {
                path: repo_root(),
                label: "repo checkout",
            },
        ]
    }

    #[test]
    fn default_falls_through_to_the_first_root_that_has_the_file() {
        let resolved = resolve_with(
            "SIM_CONFIG",
            None,
            SCENARIO_RELATIVE_DEFAULT,
            SCENARIO_HINT,
            &repo_and_missing_roots(),
        )
        .expect("the repo checkout root carries the scenario");
        assert!(resolved.ends_with(SCENARIO_RELATIVE_DEFAULT));
        assert!(Path::new(&resolved).is_file());
    }

    #[test]
    fn the_service_config_default_resolves_the_same_way() {
        let resolved = resolve_with(
            "DHALL_CONFIG",
            None,
            SERVICE_RELATIVE_DEFAULT,
            SERVICE_HINT,
            &repo_and_missing_roots(),
        )
        .expect("the repo checkout root carries the service config");
        assert!(Path::new(&resolved).is_file());
    }

    #[test]
    fn an_unresolvable_default_names_every_context_it_tried() {
        let err = resolve_with(
            "SIM_CONFIG",
            None,
            SCENARIO_RELATIVE_DEFAULT,
            SCENARIO_HINT,
            &roots(&["/nonexistent-sim-root", "/also-nonexistent"]),
        )
        .expect_err("neither root exists");
        let message = err.to_string();
        assert!(message.contains("/nonexistent-sim-root"), "{message}");
        assert!(message.contains("/also-nonexistent"), "{message}");
        assert!(message.contains(SCENARIO_HINT), "{message}");
    }

    #[test]
    fn an_explicit_path_is_used_verbatim_and_skips_the_roots() {
        let explicit = repo_root().join(SCENARIO_RELATIVE_DEFAULT);
        let resolved = resolve_with(
            "SIM_CONFIG",
            Some(explicit.to_string_lossy().into_owned()),
            SCENARIO_RELATIVE_DEFAULT,
            SCENARIO_HINT,
            &roots(&["/nonexistent-sim-root"]),
        )
        .expect("an explicit path does not consult the roots");
        assert_eq!(resolved, explicit.to_string_lossy());
    }

    #[test]
    fn an_explicit_path_that_is_missing_is_an_error_not_a_fallback() {
        let err = resolve_with(
            "SIM_CONFIG",
            Some("/no/such/scenario.dhall".to_string()),
            SCENARIO_RELATIVE_DEFAULT,
            SCENARIO_HINT,
            &repo_and_missing_roots(),
        )
        .expect_err("an explicit path must not silently fall back to a default scenario");
        assert!(err.to_string().contains("/no/such/scenario.dhall"));
    }

    #[test]
    fn an_empty_value_is_rejected_rather_than_treated_as_unset() {
        assert!(resolve_with(
            "SIM_CONFIG",
            Some("   ".to_string()),
            SCENARIO_RELATIVE_DEFAULT,
            SCENARIO_HINT,
            &repo_and_missing_roots()
        )
        .is_err());
    }
}
