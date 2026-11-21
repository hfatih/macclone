//! Read-only topology observations and the physical clone plan shared by both clients.
use super::{Disk, disks, invalid, plan};
use serde::Serialize;
use std::io;

#[cfg(any(windows, test))]
mod extents;
#[cfg(any(windows, test))]
mod locations;
#[cfg(windows)]
pub(super) mod windows;

#[derive(Debug, Serialize)]
pub struct ClonePlan {
    pub source: Disk,
    pub target: Disk,
    pub blockers: Vec<String>,
    pub physical_write_enabled: bool,
    pub topology_issues: Vec<TopologyIssue>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct TopologyIssue {
    pub code: &'static str,
    pub detail: String,
    pub disk: Option<u32>,
    pub volume: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(any(windows, test))]
enum Role {
    Windows,
    System,
    Pagefile,
    CrashDump,
}

#[cfg(any(windows, test))]
impl Role {
    fn code(self) -> &'static str {
        match self {
            Self::Windows => "windows_disk",
            Self::System => "system_disk",
            Self::Pagefile => "pagefile_disk",
            Self::CrashDump => "crash_dump_disk",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Windows => "çalışan Windows",
            Self::System => "Windows önyükleme bölümü",
            Self::Pagefile => "etkin Windows sayfa dosyası",
            Self::CrashDump => "Windows çökme dökümü",
        }
    }
}

#[cfg(any(windows, test))]
#[derive(Debug)]
struct Placement {
    role: Role,
    volume: Option<String>,
    disks: Vec<u32>,
}

#[cfg(any(windows, test))]
#[derive(Default)]
struct Topology {
    protected: Vec<Placement>,
    unresolved: Vec<TopologyIssue>,
}

#[cfg(any(windows, test))]
impl Topology {
    fn report_error(&mut self, code: &'static str, detail: String, volume: Option<String>) {
        self.unresolved.push(TopologyIssue {
            code,
            detail,
            disk: None,
            volume,
        });
    }

    fn issues(mut self, source: u32, target: u32) -> Vec<TopologyIssue> {
        for placement in self.protected {
            for disk in [source, target] {
                if placement.disks.contains(&disk) {
                    self.unresolved.push(TopologyIssue {
                        code: placement.role.code(),
                        detail: format!(
                            "Disk {disk}, {} tarafından kullanılıyor.",
                            placement.role.label()
                        ),
                        disk: Some(disk),
                        volume: placement.volume.clone(),
                    });
                }
                if source == target {
                    break;
                }
            }
        }
        self.unresolved.dedup();
        self.unresolved
    }
}

#[cfg(any(windows, test))]
fn unchanged(selected: &Disk, refreshed: &[Disk]) -> bool {
    // Health is telemetry. The other fields include identity, geometry and policy inputs.
    let mut found = refreshed.iter().filter(|d| d.number == selected.number);
    let Some(mut current) = found.next().cloned() else {
        return false;
    };
    current.health.clone_from(&selected.health);
    &current == selected && found.next().is_none()
}

pub fn plan_disks(source: u32, target: u32) -> io::Result<ClonePlan> {
    let inventory = disks()?;
    let pick = |n| {
        let mut matches = inventory.iter().filter(|d| d.number == n);
        matches
            .next()
            .filter(|_| matches.next().is_none())
            .cloned()
            .ok_or_else(|| invalid(format!("Disk {n} bulunamadı veya disk numarası belirsiz.")))
    };
    let source = pick(source)?;
    let target = pick(target)?;
    let mut blockers = plan(&source, &target);
    #[cfg(windows)]
    let topology_issues = {
        let mut topology = windows::observe(&inventory);
        match disks() {
            Ok(refreshed) => {
                for disk in [&source, &target] {
                    if !unchanged(disk, &refreshed) {
                        topology.report_error(
                            "disk_selection_changed",
                            format!(
                                "Disk {} planlama sırasında değişti veya kayboldu.",
                                disk.number
                            ),
                            None,
                        );
                    }
                }
            }
            Err(error) => topology.report_error(
                "disk_inventory_unavailable",
                format!("Disk seçimi yeniden doğrulanamadı ({error})."),
                None,
            ),
        }
        topology.issues(source.number, target.number)
    };
    #[cfg(not(windows))]
    let topology_issues: Vec<TopologyIssue> = Vec::new();
    blockers.extend(topology_issues.iter().map(|issue| issue.detail.clone()));
    blockers.dedup();
    Ok(ClonePlan {
        source,
        target,
        blockers,
        physical_write_enabled: false,
        topology_issues,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::disk;

    #[test]
    fn windows_and_efi_on_separate_disks_and_letterless_spanned_volume() {
        let topology = Topology {
            protected: vec![
                Placement {
                    role: Role::Windows,
                    volume: Some("windows-guid".into()),
                    disks: vec![1],
                },
                Placement {
                    role: Role::System,
                    volume: Some("letterless-efi-guid".into()),
                    disks: vec![7, 2],
                },
            ],
            ..Default::default()
        };
        let issues = topology.issues(1, 2);
        assert_eq!(issues[0].code, "windows_disk");
        assert_eq!(issues[0].disk, Some(1));
        assert_eq!(issues[1].code, "system_disk");
        assert_eq!(issues[1].volume.as_deref(), Some("letterless-efi-guid"));
        assert_eq!(issues[1].disk, Some(2));
    }

    #[test]
    fn pagefile_dump_and_unknown_locations_block() {
        let mut topology = Topology {
            protected: vec![
                Placement {
                    role: Role::Pagefile,
                    volume: None,
                    disks: vec![9, 2],
                },
                Placement {
                    role: Role::CrashDump,
                    volume: None,
                    disks: vec![2],
                },
            ],
            ..Default::default()
        };
        topology.report_error("location_unresolved", "Access denied".into(), None);
        let codes: Vec<_> = topology
            .issues(1, 2)
            .into_iter()
            .map(|issue| issue.code)
            .collect();
        assert_eq!(
            codes,
            ["location_unresolved", "pagefile_disk", "crash_dump_disk"]
        );
    }

    #[test]
    fn disjoint_placements_and_stable_selection() {
        let topology = Topology {
            protected: vec![Placement {
                role: Role::Windows,
                volume: None,
                disks: vec![9, 10],
            }],
            ..Default::default()
        };
        let source = disk(1);
        let target = disk(2);
        assert!(plan(&source, &target).is_empty());
        assert!(topology.issues(source.number, target.number).is_empty());
        let mut changed = target.clone();
        changed.id = Some("replacement".into());
        assert!(!unchanged(&target, &[changed]));
        assert!(!unchanged(&target, &[]));
        assert!(unchanged(&target, &[target.clone(), source]));
    }
}
