use crate::{config::RunnerConfig, journal::Journal};
use anyhow::Result;
use bollard::{Docker, container::ListContainersOptions};
use std::collections::HashMap;
pub async fn cleanup(c: &RunnerConfig) -> Result<()> {
    let d = Docker::connect_with_local_defaults()?;
    let filters = HashMap::from([(
        "label".to_string(),
        vec![
            "omniroute.managed=true".to_string(),
            format!("omniroute.runner_id={}", c.runner_id()),
        ],
    )]);
    let items = d
        .list_containers(Some(ListContainersOptions {
            all: true,
            filters,
            ..Default::default()
        }))
        .await?;
    for x in items {
        if let Some(id) = x.id {
            let _ = d
                .remove_container(
                    &id,
                    Some(bollard::container::RemoveContainerOptions {
                        force: true,
                        ..Default::default()
                    }),
                )
                .await;
        }
    }
    let j = Journal::open(&c.work_dir.join("runner.db"))?;
    for (id, a) in j.unfinished()? {
        let _ = j.transition(&id, a, crate::state::State::Destroying);
        let _ = j.transition(&id, a, crate::state::State::Destroyed);
    }
    Ok(())
}
