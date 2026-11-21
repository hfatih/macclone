use super::{Role, Topology};
use serde::Deserialize;

#[derive(Deserialize)]
pub(super) struct Locations {
    pagefiles: Vec<Option<String>>,
    pagefile_error: Option<String>,
    crash: Option<CrashConfig>,
    crash_error: Option<String>,
}

#[derive(Deserialize)]
struct CrashConfig {
    mode: Option<u32>,
    dedicated: Option<String>,
    dump_file: Option<String>,
    minidump_dir: Option<String>,
}

impl Locations {
    pub(super) fn observe(
        self,
        topology: &mut Topology,
        mut place: impl FnMut(&mut Topology, Role, &str),
    ) {
        if let Some(error) = self.pagefile_error {
            topology.report_error(
                "pagefiles_unavailable",
                format!("Etkin sayfa dosyaları alınamadı ({error})."),
                None,
            );
        }
        for path in &self.pagefiles {
            place(topology, Role::Pagefile, path.as_deref().unwrap_or(""));
        }
        match self.crash {
            Some(CrashConfig { mode: Some(0), .. }) => (),
            Some(
                c @ CrashConfig {
                    mode: Some(1 | 2 | 3 | 7),
                    ..
                },
            ) => {
                let dedicated = c.dedicated.as_deref().filter(|s| !s.trim().is_empty());
                if let Some(path) = dedicated {
                    place(topology, Role::CrashDump, path);
                } else if self.pagefiles.is_empty() {
                    topology.report_error(
                        "crash_backing_unresolved",
                        "Etkin çökme dökümünün disk üzerindeki yazma alanı belirlenemedi.".into(),
                        None,
                    );
                }
                let output = if c.mode == Some(3) {
                    c.minidump_dir
                        .as_deref()
                        .unwrap_or(r"%SystemRoot%\Minidump")
                } else {
                    c.dump_file.as_deref().unwrap_or(r"%SystemRoot%\MEMORY.DMP")
                };
                place(topology, Role::CrashDump, output);
            }
            _ => {
                let error = self
                    .crash_error
                    .as_deref()
                    .unwrap_or("döküm ayarı eksik veya desteklenmiyor");
                topology.report_error(
                    "crash_configuration_unavailable",
                    format!("Çökme dökümü yapılandırması doğrulanamadı ({error})."),
                    None,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_dump_and_absent_pagefiles_require_no_placements() {
        let obs: Locations =
            serde_json::from_str(r#"{"pagefiles":[],"crash":{"mode":0}}"#).unwrap();
        let mut topology = Topology::default();
        obs.observe(&mut topology, |_, _, _| {
            panic!("An unset location should not be resolved")
        });
        assert!(topology.issues(1, 2).is_empty());
    }

    #[test]
    fn missing_dump_backing_differs_from_disabled_dump() {
        let obs: Locations =
            serde_json::from_str(r#"{"pagefiles":[],"crash":{"mode":7}}"#).unwrap();
        let mut topology = Topology::default();
        obs.observe(&mut topology, |_, role, path| {
            assert_eq!(role, Role::CrashDump);
            assert_eq!(path, r"%SystemRoot%\MEMORY.DMP");
        });
        assert_eq!(topology.issues(1, 2)[0].code, "crash_backing_unresolved");
    }

    #[test]
    fn active_pagefile_and_both_dump_paths_are_resolved() {
        let obs: Locations = serde_json::from_str(
            r#"{"pagefiles":["E:\\pagefile.sys"],"crash":{"mode":3,"dedicated":"F:\\dedicated.sys","minidump_dir":"G:\\Dumps"}}"#,
        ).unwrap();
        let mut topology = Topology::default();
        let mut resolved = Vec::new();
        obs.observe(&mut topology, |_, role, path| {
            resolved.push((role, path.to_owned()))
        });
        assert_eq!(
            resolved,
            [
                (Role::Pagefile, r"E:\pagefile.sys".to_owned()),
                (Role::CrashDump, r"F:\dedicated.sys".to_owned()),
                (Role::CrashDump, r"G:\Dumps".to_owned()),
            ]
        );
        assert!(topology.issues(1, 2).is_empty());
    }
}
