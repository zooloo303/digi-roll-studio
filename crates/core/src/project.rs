// The project file: a session on disk, as JSON.
//
// `core` does no I/O (PLAN.md §3), so this converts to and from a string and
// leaves opening files to the caller.

use serde::{Deserialize, Serialize};

use crate::device::{DeviceId, PortRef};
use crate::model::ModelError;
use crate::session::Session;

/// Bumped only for a change a previous build could not read correctly. Adding a
/// field with `#[serde(default)]` does not need it.
pub const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub format: u32,
    pub session: Session,
}

impl Project {
    pub fn new(session: Session) -> Self {
        Self {
            format: FORMAT_VERSION,
            session,
        }
    }

    pub fn to_json(&self) -> Result<String, ProjectError> {
        serde_json::to_string(self).map_err(ProjectError::Json)
    }

    pub fn to_json_pretty(&self) -> Result<String, ProjectError> {
        serde_json::to_string_pretty(self).map_err(ProjectError::Json)
    }

    /// Parse, then check the session is coherent before handing it back.
    ///
    /// A file whose track counts disagree with its device models is rejected
    /// rather than repaired: silently padding a pattern to 16 tracks would
    /// invent tracks that were never played, and silently truncating would throw
    /// notes away.
    pub fn from_json(json: &str) -> Result<Self, ProjectError> {
        let project: Self = serde_json::from_str(json).map_err(ProjectError::Json)?;
        if project.format > FORMAT_VERSION {
            return Err(ProjectError::FromTheFuture {
                found: project.format,
                supported: FORMAT_VERSION,
            });
        }
        project.session.validate().map_err(ProjectError::Model)?;
        // Device ids came off disk, so the in-process counter must not hand out
        // one of them again to a device added later.
        DeviceId::reserve_past(project.session.highest_device_id());
        Ok(project)
    }

    /// Load, then re-point every device at the ports actually connected now.
    ///
    /// Returns the devices whose remembered ports are gone. Their patterns are
    /// untouched — a missing box costs you its I/O and nothing else.
    pub fn from_json_with_ports(
        json: &str,
        available_in: &[PortRef],
        available_out: &[PortRef],
    ) -> Result<(Self, Vec<DeviceId>), ProjectError> {
        let mut project = Self::from_json(json)?;
        let unbound = project.session.rebind_ports(available_in, available_out);
        Ok((project, unbound))
    }
}

#[derive(Debug)]
pub enum ProjectError {
    Json(serde_json::Error),
    Model(ModelError),
    /// Written by a newer build than this one.
    FromTheFuture { found: u32, supported: u32 },
}

impl std::fmt::Display for ProjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(e) => write!(f, "project file is not readable: {e}"),
            Self::Model(e) => write!(f, "project file is inconsistent: {e}"),
            Self::FromTheFuture { found, supported } => write!(
                f,
                "project file is format {found}, but this build reads up to {supported}"
            ),
        }
    }
}

impl std::error::Error for ProjectError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(e) => Some(e),
            Self::Model(e) => Some(e),
            Self::FromTheFuture { .. } => None,
        }
    }
}
