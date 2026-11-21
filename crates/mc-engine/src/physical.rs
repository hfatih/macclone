use super::*;
use mc_device::{ApprovedClone, CloneLayout, PhysicalTarget};

pub fn clone_physical(
    approved: ApprovedClone,
    options: Options,
    resume: bool,
    cancel: &AtomicBool,
    progress: impl FnMut(Progress),
) -> io::Result<Report> {
    if options.recovery {
        return Err(invalid("Use RAW imaging for a failing source"));
    }
    checkpoint(cancel)?;
    let plan = approved.plan();
    options.validate(plan.source.physical.max(plan.target.physical))?;
    let input = Input::Disk {
        number: plan.source.number,
        id: plan
            .source
            .id
            .clone()
            .ok_or_else(|| invalid("Missing approved source identity"))?,
    };
    let layout = plan.layout;
    let session = approved.prepare_journal(resume)?;
    let directory = session.job().to_owned();
    let job = Journal::open(&directory, resume)?;
    let (mut source, mut target, _held_session) =
        session.open([&job.manifest, &job.map])?.into_parts();
    let gpt = match layout {
        CloneLayout::Exact if source.stamp.bytes == target.capacity() => None,
        CloneLayout::Exact => return Err(invalid("Physical clone endpoint capacity changed")),
        CloneLayout::RelocateGpt => Some(mc_inspect::GptRelocation::prepare(
            &mut source.reader,
            source.stamp.bytes,
            target.capacity(),
            source.stamp.sector,
        )?),
    };
    transfer(
        &input,
        TransferSource {
            original: source,
            gpt,
        },
        &mut target,
        job,
        options,
        cancel,
        progress,
    )
}

impl Durable for PhysicalTarget {
    fn durable(&mut self) -> io::Result<()> {
        self.sync()
    }
}

impl Target for PhysicalTarget {
    type Snapshot = ();
    const REPORT_DEVICE_IDENTITY: bool = true;
    fn identity(&self) -> io::Result<String> {
        Ok(PhysicalTarget::identity(self).to_owned())
    }
    fn length(&self) -> io::Result<u64> {
        Ok(self.capacity())
    }
    fn snapshot(&self, total: u64) -> io::Result<()> {
        if self.reported_length()? != total {
            return Err(invalid("Physical destination capacity changed"));
        }
        self.assert_writable_offline()
    }
    fn check_snapshot(&self, total: u64, (): ()) -> io::Result<()> {
        self.snapshot(total)
    }
}
