use super::{ClonePlan, unchanged};
use crate::{Disk, direct_transport, invalid};
use std::{io, time::Duration};

#[cfg(windows)]
use crate::{
    Source,
    win::{PhysicalTarget, open_clone_handles},
};
#[cfg(windows)]
use std::{
    fs::File,
    path::{Path, PathBuf},
    time::Instant,
};

const APPROVAL_TTL: Duration = Duration::from_secs(5 * 60);

fn eligible(plan: &ClonePlan) -> io::Result<()> {
    if plan.blockers.is_empty() {
        Ok(())
    } else {
        Err(invalid(plan.blockers.join("\n")))
    }
}

fn recheck(before: &ClonePlan, current: &ClonePlan) -> io::Result<()> {
    eligible(current)?;
    if !unchanged(&before.source, std::slice::from_ref(&current.source))
        || !unchanged(&before.target, std::slice::from_ref(&current.target))
    {
        return Err(invalid("Source or destination changed since confirmation"));
    }
    Ok(())
}

fn confirm(expected: &str, typed: &str, elapsed: Duration) -> io::Result<()> {
    if elapsed > APPROVAL_TTL {
        return Err(invalid("Confirmation expired. Create a fresh clone plan."));
    }
    if expected != typed {
        return Err(invalid(
            "Confirmation did not match the selected destination",
        ));
    }
    Ok(())
}

#[cfg(windows)]
pub struct PreparedClone {
    plan: ClonePlan,
    confirmation: String,
    created: Instant,
    parent: PathBuf,
    parent_handle: File,
    job: PathBuf,
}

#[cfg(windows)]
impl PreparedClone {
    pub fn new(
        source: u32,
        target: u32,
        job: &Path,
        layout: super::CloneLayout,
    ) -> io::Result<Self> {
        let plan = super::plan_disks(source, target)?;
        eligible(&plan)?;
        if plan.layout != layout {
            return Err(invalid(match plan.layout {
                super::CloneLayout::Exact => {
                    "Equal-capacity disks require exact cloning without --relocate-gpt"
                }
                super::CloneLayout::RelocateGpt => {
                    "Larger destination requires explicit --relocate-gpt"
                }
            }));
        }
        let name = job
            .file_name()
            .ok_or_else(|| invalid("Missing journal name"))?;
        crate::local_path(job)?;
        let parent = job
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let parent_handle = super::windows::hold_directory(parent)?;
        check_journal(&parent_handle, &plan, &crate::disks()?)?;
        let mut random = [0; 16];
        getrandom::fill(&mut random).map_err(io::Error::other)?;
        let nonce = u128::from_be_bytes(random);
        Ok(Self {
            plan,
            confirmation: format!("CLONE DISK {target} {nonce:032X}"),
            created: Instant::now(),
            parent: parent.to_owned(),
            parent_handle,
            job: parent.join(name),
        })
    }

    pub fn plan(&self) -> &ClonePlan {
        &self.plan
    }
    pub fn confirmation(&self) -> &str {
        &self.confirmation
    }

    pub fn approve(self, entered: &str) -> io::Result<ApprovedClone> {
        confirm(&self.confirmation, entered, self.created.elapsed())?;
        Ok(ApprovedClone(self))
    }
}

#[cfg(windows)]
pub struct ApprovedClone(PreparedClone);

#[cfg(windows)]
impl ApprovedClone {
    pub fn plan(&self) -> &ClonePlan {
        &self.0.plan
    }

    pub fn prepare_journal(self, resume: bool) -> io::Result<PhysicalSession> {
        let plan = &self.0.plan;
        let inventory = crate::disks()?;
        check_journal(&self.0.parent_handle, plan, &inventory)?;
        let named_parent = super::windows::hold_directory(&self.0.parent)?;
        if crate::file_id(&named_parent)? != crate::file_id(&self.0.parent_handle)? {
            return Err(invalid("Journal directory moved during plan confirmation"));
        }
        if !resume {
            std::fs::create_dir(&self.0.job)?;
        }
        let dir = super::windows::hold_directory(&self.0.job)?;
        check_journal(&dir, plan, &inventory)?;
        Ok(PhysicalSession {
            approval: self,
            _dir: dir,
        })
    }
}

#[cfg(windows)]
pub struct PhysicalSession {
    approval: ApprovedClone,
    _dir: File,
}

#[cfg(windows)]
impl PhysicalSession {
    pub fn job(&self) -> &Path {
        &self.approval.0.job
    }

    pub fn open(self, files: [&File; 2]) -> io::Result<OpenPhysicalSession> {
        let before = &self.approval.0.plan;
        let fresh = super::plan_disks(before.source.number, before.target.number)?;
        recheck(before, &fresh)?;
        let (source, target) = open_clone_handles(&fresh.source, &fresh.target)?;
        let after = super::plan_disks(before.source.number, before.target.number)?;
        recheck(before, &after)?;
        let inventory = crate::disks()?;
        if [&before.source, &before.target]
            .iter()
            .any(|disk| !unchanged(disk, &inventory) || super::identity_conflict(disk, &inventory))
        {
            return Err(invalid("Clone endpoints changed during journal validation"));
        }
        for file in files {
            check_journal(file, before, &inventory)?;
        }
        target.assert_writable_offline()?;
        if self.approval.0.created.elapsed() > APPROVAL_TTL {
            return Err(invalid("Confirmation expired during device revalidation"));
        }
        Ok(OpenPhysicalSession {
            source,
            target,
            session: self,
        })
    }
}

#[cfg(windows)]
fn check_journal(file: &File, plan: &ClonePlan, inventory: &[Disk]) -> io::Result<()> {
    let disks = super::windows::journal_disks(file)?;
    journal_is_disjoint(&disks, plan, inventory)
}

fn journal_is_disjoint(disks: &[u32], plan: &ClonePlan, inventory: &[Disk]) -> io::Result<()> {
    if disks.is_empty()
        || disks.contains(&plan.source.number)
        || disks.contains(&plan.target.number)
    {
        return Err(invalid("Journal must reside outside both clone devices"));
    }
    for number in disks {
        let mut devices = inventory.iter().filter(|disk| disk.number == *number);
        if !devices
            .next()
            .is_some_and(|device| direct_transport(&device.bus))
            || devices.next().is_some()
        {
            return Err(invalid(
                "Journal backing disks must resolve to direct physical storage",
            ));
        }
    }
    Ok(())
}

#[cfg(windows)]
pub struct OpenPhysicalSession {
    source: Source,
    target: PhysicalTarget,
    // Retain directory and parent handles while the raw device handles are in use.
    session: PhysicalSession,
}

#[cfg(windows)]
impl OpenPhysicalSession {
    pub fn into_parts(self) -> (Source, PhysicalTarget, PhysicalSession) {
        (self.source, self.target, self.session)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::disk;

    fn plan() -> ClonePlan {
        ClonePlan {
            layout: super::super::CloneLayout::Exact,
            source: disk(1),
            target: disk(2),
            blockers: vec![],
            physical_write_enabled: false,
            topology_issues: vec![],
        }
    }

    #[test]
    fn confirmation_is_exact_and_expires() {
        let fresh = Duration::ZERO;
        assert!(confirm("CLONE DISK 2 AB", "CLONE DISK 2 AB", fresh).is_ok());
        assert!(confirm("CLONE DISK 2 AB", "CLONE DISK 1 AB", fresh).is_err());
        assert!(
            confirm(
                "CLONE DISK 2 AB",
                "CLONE DISK 2 AB",
                APPROVAL_TTL + Duration::from_secs(1)
            )
            .is_err()
        );
    }

    #[test]
    fn journal_must_be_on_other_storage_for_every_extent() {
        let plan = plan();
        let known = [disk(7), disk(8)];
        assert!(journal_is_disjoint(&[7, 8], &plan, &known).is_ok());
        assert!(journal_is_disjoint(&[7, plan.source.number], &plan, &known).is_err());
        assert!(journal_is_disjoint(&[7, plan.target.number], &plan, &known).is_err());
        assert!(journal_is_disjoint(&[], &plan, &known).is_err());
    }

    #[test]
    fn virtual_missing_or_ambiguous_journal_backings_are_rejected() {
        let plan = plan();
        let mut virtual_disk = disk(8);
        virtual_disk.bus = "File Backed Virtual".into();
        assert!(journal_is_disjoint(&[7, 8], &plan, &[disk(7), virtual_disk]).is_err());
        assert!(journal_is_disjoint(&[7, 8], &plan, &[disk(7)]).is_err());
        assert!(journal_is_disjoint(&[7, 8], &plan, &[disk(7), disk(8), disk(8)]).is_err());
    }

    #[test]
    fn refreshed_plan_rejects_replaced_disks_and_new_blockers() {
        let before = plan();
        let mut after = plan();
        assert!(recheck(&before, &after).is_ok());
        after.target.serial = Some("REPLACEMENT".into());
        assert!(recheck(&before, &after).is_err());
        after = plan();
        after.blockers.push("Protected volume appeared".into());
        assert!(recheck(&before, &after).is_err());
    }
}
