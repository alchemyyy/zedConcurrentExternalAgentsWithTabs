use std::{collections::HashMap, path::Path, process::Output, sync::Arc};

use serde::{Deserialize, Serialize};
use smol::process::Command;

use crate::{DevContainerConfig, devcontainer_api::DevContainerUp};

#[derive(Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
struct DevContainer {
    image: Option<String>,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum RenameMeError {
    DevContainerParseFailed,
    UnableToInspectDockerImage, // TODO maybe not needed eventually
    UnmappedError,
}

#[derive(Debug, Deserialize, Serialize, Eq, PartialEq)]
struct DockerConfigLabels {
    #[serde(rename = "devcontainer.metadata")]
    metadata: Option<Vec<HashMap<String, String>>>,
}

#[derive(Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "PascalCase")]
struct DockerInspectConfig {
    labels: DockerConfigLabels,
}

#[derive(Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "PascalCase")]
struct DockerInspectTodoRename {
    config: DockerInspectConfig,
}

#[derive(Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "PascalCase")]
struct DockerPs {
    #[serde(rename = "ID")]
    id: String,
}

// TODO podman
fn docker_cli() -> &'static str {
    "docker"
}

// Main entrypoint for this effort
// Wait big question: how do I get the container_id back?
// Based on the CLI implementation: filter by labels
// Which labels?
/**
*   --id-label                        Id label(s) of the format name=value. These will be set on the container and used to
                                   query for an existing container. If no --id-label is given, one will be inferred fr
                                  om the --workspace-folder path.
* So a) we can provide one. But if not:
* 		container = await findDevContainer(params, [`${hostFolderLabel}=${workspaceFolder}`, `${configFileLabel}=${configFile}`]);
        if (!container) {
            // Fall back to old labels.
            container = await findDevContainer(params, [`${hostFolderLabel}=${workspaceFolder}`]);
* So basically find by devcontainer.local_folder and devcontainer.config_file
* So we're going to need to add these to the args in the docker run test
* Do we have to care about the name in docker world?
*/
pub(crate) async fn spawn_dev_container(
    config: DevContainerConfig,
    local_project_path: Arc<&Path>,
) -> Result<DevContainerUp, RenameMeError> {
    let mut labels = HashMap::new();
    labels.insert(
        "devcontainer.local_folder",
        local_project_path.display().to_string(),
    );
    labels.insert(
        "devcontainer.config_file",
        config.config_path.display().to_string(),
    );

    let devcontainer = deserialize_devcontainer_json(
        &std::fs::read_to_string(local_project_path.join(config.config_path)).expect("todo"),
    )?;

    let Ok(mut command) = create_docker_query_containers(Some(labels)) else {
        return Err(RenameMeError::UnmappedError);
    };

    let Ok(output) = command.output().await else {
        return Err(RenameMeError::UnmappedError);
    };

    // Execute command, get back ids (or not)
    let docker_ps: Option<DockerPs> = deserialize_json_output(output)?;

    if docker_ps.is_none() {
        // Arg this comes too early. Before anything else, I need to parse that JSON
        let docker_run_command = create_docker_run_command(&devcontainer, local_project_path)?;
    }

    // If not, create with docker run
    // Either way:
    //   Inspect
    //   If started, return details
    //   If unstarted, start somehow

    // Err(RenameMeError::UnmappedError)
    Ok(DevContainerUp {
        _outcome: "todo".to_string(),
        container_id: "todo, get from query command".to_string(),
        remote_user: "todo, get from remote-user function".to_string(),
        remote_workspace_folder: "todo, get from mounts (function needed".to_string(),
    })
}

// For this to work, I have to ignore quiet and instead do format=json
fn deserialize_json_output<T>(output: Output) -> Result<Option<T>, RenameMeError>
where
    T: for<'de> Deserialize<'de>,
{
    if output.status.success() {
        let raw = String::from_utf8_lossy(&output.stdout);
        if raw.is_empty() {
            return Ok(None);
        }
        serde_json::from_str(&raw).map_err(|e| {
            dbg!(&e);
            RenameMeError::UnmappedError
        })
    } else {
        Err(RenameMeError::UnmappedError)
    }
}

fn deserialize_devcontainer_json(json: &str) -> Result<DevContainer, RenameMeError> {
    match serde_json::from_str(json) {
        Ok(devcontainer) => Ok(devcontainer),
        Err(_) => Err(RenameMeError::DevContainerParseFailed),
    }
}

fn docker_pull_for_devcontainer(devcontainer: &DevContainer) -> Result<Command, RenameMeError> {
    let Some(image) = &devcontainer.image else {
        return Err(RenameMeError::UnableToInspectDockerImage);
    };
    let mut command = smol::process::Command::new(docker_cli());
    command.args(&["pull", image]);
    Ok(command)
}

fn create_docker_inspect_for_image(devcontainer: &DevContainer) -> Result<Command, RenameMeError> {
    let Some(image) = &devcontainer.image else {
        return Err(RenameMeError::UnableToInspectDockerImage);
    };
    let mut command = smol::process::Command::new(docker_cli());
    command.args(&["inspect", image]);
    Ok(command)
}

fn create_docker_query_containers(
    filter_labels: Option<HashMap<&str, String>>, // This should be a hashmap
) -> Result<Command, RenameMeError> {
    let mut command = smol::process::Command::new(docker_cli());
    command.args(&["ps", "-q", "-a"]);

    if let Some(labels) = filter_labels {
        for (key, value) in labels {
            command.arg("--filter");
            command.arg(format!("label={key}={value}"));
        }
    }
    Ok(command)
}

fn create_docker_run_command(
    devcontainer: &DevContainer,
    local_project_directory: Arc<&Path>,
) -> Result<Command, RenameMeError> {
    let Some(image) = &devcontainer.image else {
        return Err(RenameMeError::UnmappedError);
    };
    // let remote_user = get_remote_user_from_config(config)?;

    let Some(project_directory) = local_project_directory.file_name() else {
        return Err(RenameMeError::UnmappedError);
    };
    let remote_workspace_folder = format!("/workspaces/{}", project_directory.display()); // TODO workspaces should be overridable

    let mut command = Command::new(docker_cli());

    // TODO TODO
    command.arg("run");
    command.arg("--sig-proxy=false");
    command.arg("-a");
    command.arg("STDOUT");
    command.arg("-a");
    command.arg("STDERR");
    command.arg("--mount");
    command.arg(format!(
        "type=bind,source={},target={},consistency=cached",
        local_project_directory.display(),
        remote_workspace_folder
    ));
    command.arg("--entrypoint");
    command.arg("/bin/sh");
    command.arg(image);
    command.arg("-c");
    command.arg(
        "
echo Container started
trap \"exit 0\" 15
exec \"$@\"
while sleep 1 & wait $!; do :; done
        "
        .trim(),
    );
    command.arg("-");

    Ok(command)
}

fn get_remote_user_from_config(config: &DockerInspectTodoRename) -> Result<String, RenameMeError> {
    let Some(metadata) = &config.config.labels.metadata else {
        return Err(RenameMeError::UnmappedError);
    };
    for metadatum in metadata {
        if let Some(remote_user) = metadatum.get("remoteUser") {
            return Ok(remote_user.to_string());
        }
    }
    Err(RenameMeError::UnmappedError)
}

#[cfg(test)]
mod test {
    use std::{
        collections::HashMap,
        ffi::OsStr,
        path::Path,
        process::{ExitStatus, Output},
        sync::Arc,
    };

    use smol::process::Command;

    use crate::model::{
        DevContainer, DockerConfigLabels, DockerInspectConfig, DockerInspectTodoRename, DockerPs,
        RenameMeError, create_docker_inspect_for_image, create_docker_run_command,
        deserialize_devcontainer_json, deserialize_json_output, docker_pull_for_devcontainer,
        get_remote_user_from_config,
    };

    #[test]
    fn should_deserialize_simple_devcontainer_json() {
        let given_bad_json = "{ \"image\": 123 }";

        let result: Result<DevContainer, RenameMeError> =
            deserialize_devcontainer_json(given_bad_json);

        assert!(result.is_err());
        assert_eq!(
            result.expect_err("err"),
            RenameMeError::DevContainerParseFailed
        );

        let given_good_json = "{\"image\": \"mcr.microsoft.com/devcontainers/base:ubuntu\"}";

        let result: Result<DevContainer, RenameMeError> =
            deserialize_devcontainer_json(given_good_json);

        assert!(result.is_ok());
        assert_eq!(
            result.expect("ok"),
            DevContainer {
                image: Some(String::from("mcr.microsoft.com/devcontainers/base:ubuntu"))
            }
        );
    }

    #[test]
    fn should_create_docker_inspect_command() {
        let given_devcontainer = DevContainer {
            image: Some("mcr.microsoft.com/devcontainers/base:ubuntu".to_string()),
        };

        let docker_pull_command = docker_pull_for_devcontainer(&given_devcontainer);
        assert!(docker_pull_command.is_ok());
        let docker_pull_command = docker_pull_command.expect("ok");

        assert_eq!(docker_pull_command.get_program(), "docker");
        assert_eq!(
            docker_pull_command.get_args().collect::<Vec<&OsStr>>(),
            vec![
                OsStr::new("pull"),
                OsStr::new("mcr.microsoft.com/devcontainers/base:ubuntu")
            ]
        );

        let docker_inspect_command = create_docker_inspect_for_image(&given_devcontainer);

        assert!(docker_inspect_command.is_ok());
        let docker_inspect_command = docker_inspect_command.expect("ok");

        assert_eq!(docker_inspect_command.get_program(), "docker");
        assert_eq!(
            docker_inspect_command.get_args().collect::<Vec<&OsStr>>(),
            vec![
                OsStr::new("inspect"),
                OsStr::new("mcr.microsoft.com/devcontainers/base:ubuntu")
            ]
        )
    }

    #[test]
    fn should_get_remote_user_from_devcontainer_config() {
        let mut metadata = HashMap::new();
        metadata.insert("remoteUser".to_string(), "vsCode".to_string());
        let given_docker_config = DockerInspectTodoRename {
            config: DockerInspectConfig {
                labels: DockerConfigLabels {
                    metadata: Some(vec![metadata]),
                },
            },
        };

        let remote_user = get_remote_user_from_config(&given_docker_config);

        assert!(remote_user.is_ok());
        let remote_user = remote_user.expect("ok");
        assert_eq!(&remote_user, "vsCode")
    }

    #[test]
    fn should_create_correct_docker_run_command() {
        let mut metadata = HashMap::new();
        metadata.insert("remoteUser".to_string(), "vsCode".to_string());
        let given_docker_config = DockerInspectTodoRename {
            config: DockerInspectConfig {
                labels: DockerConfigLabels {
                    metadata: Some(vec![metadata]),
                },
            },
        };
        let given_devcontainer = DevContainer {
            image: Some("mcr.microsoft.com/devcontainers/base:ubuntu".to_string()),
        };

        let docker_run_command = create_docker_run_command(
            &given_devcontainer,
            Arc::new(Path::new("/local/project_app")),
        );

        assert!(docker_run_command.is_ok());
        let docker_run_command = docker_run_command.expect("ok");

        assert_eq!(docker_run_command.get_program(), "docker");
        assert_eq!(
            docker_run_command.get_args().collect::<Vec<&OsStr>>(),
            vec![
                OsStr::new("run"),
                OsStr::new("--sig-proxy=false"),
                OsStr::new("-a"),
                OsStr::new("STDOUT"),
                OsStr::new("-a"),
                OsStr::new("STDERR"),
                OsStr::new("--mount"),
                OsStr::new(
                    "type=bind,source=/local/project_app,target=/workspaces/project_app,consistency=cached"
                ),
                OsStr::new("--entrypoint"),
                OsStr::new("/bin/sh"),
                OsStr::new("mcr.microsoft.com/devcontainers/base:ubuntu"),
                OsStr::new("-c"),
                OsStr::new(
                    "
echo Container started
trap \"exit 0\" 15
exec \"$@\"
while sleep 1 & wait $!; do :; done
                    "
                    .trim()
                ),
                OsStr::new("-"),
            ]
        )
    }

    #[test]
    fn should_deserialize_docker_ps_with_filters() {
        // First, deserializes empty
        let empty_output = Output {
            status: ExitStatus::default(),
            stderr: vec![],
            stdout: String::from("").into_bytes(),
        };

        let result: Option<DockerPs> = deserialize_json_output(empty_output).unwrap();

        assert!(result.is_none());

        let full_output = Output {
            status: ExitStatus::default(),
            stderr: vec![],
            stdout: String::from(r#"
{
    "Command": "\"/bin/sh -c 'echo Co…\"",
    "CreatedAt": "2026-02-04 15:44:21 -0800 PST",
    "ID": "abdb6ab59573",
    "Image": "mcr.microsoft.com/devcontainers/base:ubuntu",
    "Labels": "desktop.docker.io/mounts/0/Source=/Users/kylebarton/Source/OSSProjects/cli,desktop.docker.io/mounts/0/SourceKind=hostFile,desktop.docker.io/mounts/0/Target=/workspaces/cli,desktop.docker.io/ports.scheme=v2,dev.containers.features=common,dev.containers.id=base-ubuntu,dev.containers.release=v0.4.24,dev.containers.source=https://github.com/devcontainers/images,dev.containers.timestamp=Fri, 30 Jan 2026 16:52:34 GMT,dev.containers.variant=noble,devcontainer.config_file=/Users/kylebarton/Source/OSSProjects/cli/.devcontainer/dev_container_2/devcontainer.json,devcontainer.local_folder=/Users/kylebarton/Source/OSSProjects/cli,devcontainer.metadata=[{\"id\":\"ghcr.io/devcontainers/features/common-utils:2\"},{\"id\":\"ghcr.io/devcontainers/features/git:1\",\"customizations\":{\"vscode\":{\"settings\":{\"github.copilot.chat.codeGeneration.instructions\":[{\"text\":\"This dev container includes an up-to-date version of Git, built from source as needed, pre-installed and available on the `PATH`.\"}]}}}},{\"remoteUser\":\"vscode\"}],org.opencontainers.image.ref.name=ubuntu,org.opencontainers.image.version=24.04,version=2.1.6",
    "LocalVolumes": "0",
    "Mounts": "/host_mnt/User…",
    "Names": "objective_haslett",
    "Networks": "bridge",
    "Platform": {
    "architecture": "arm64",
    "os": "linux"
    },
    "Ports": "",
    "RunningFor": "47 hours ago",
    "Size": "0B",
    "State": "running",
    "Status": "Up 47 hours"
}
                "#).into_bytes(),
        };

        let result: Option<DockerPs> = deserialize_json_output(full_output).unwrap();

        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.id, "abdb6ab59573".to_string());
    }
    // Next, create relevant docker command
    //
    // Next, create appropriate response to user
}
