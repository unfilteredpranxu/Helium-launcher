//! Owner-thread polling. The completion block is ONLY an owned keepalive,
//! never a status publisher; it captures no command, buffer, texture, or authority.
use super::*;
use crate::upload::{Ownership, Poll};
use block2::RcBlock;

type Command = Retained<ProtocolObject<dyn MTLCommandBuffer>>;
struct KeepAlive<S> {
    owners: Ownership<S>,
    // These two protocols are explicitly Send + Sync in objc2-metal. GPU resources
    // are retained by queue.commandBuffer(), NOT moved to a Rust cleanup thread.
    _device: Retained<Device>,
    _queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
}
pub(super) struct Pending<S: Source> {
    command: Command,
    _staging: Buffer,
    _texture: Texture,
    lease: Arc<KeepAlive<S>>,
    description: Description<S::Generation>,
    revoked: Option<String>,
}
impl<S: Source + Send + Sync + 'static> Material<S> {
    /// Validates, encodes and commits ONE upload. Never waits for scheduling or
    /// completion. Busy/rejected calls allocate no staging or command buffer.
    pub fn begin_upload(
        &mut self,
        renderer: &TerrainRenderer,
        source: S,
        authority: &S::Authority,
        update: Update,
        budget: Budget,
    ) -> Result<(), String> {
        if self.pending.is_some() {
            return Err("material upload already pending".into());
        }
        if self.device_id != renderer.upload_device || self.texture.is_none() {
            return Err("released material or wrong/recreated device owner".into());
        }
        let permit = renderer.upload_slot.acquire()?;
        let plan = Plan::prepare(
            &source,
            authority,
            self.source.as_deref(),
            update,
            &renderer.capabilities,
            budget,
        )
        .inspect_err(|e| {
            if e.contains("budget") {
                self.counters.budget_rejections = self.counters.budget_rejections.saturating_add(1);
            }
        })?;
        let d = plan.description();
        let mut expected = self.description;
        expected.identity.revision = d.identity.revision;
        if expected != d
            || self
                .source
                .as_ref()
                .is_some_and(|old| old.description().identity.revision >= d.identity.revision)
        {
            return Err("incompatible generation/layout or stale upload revision".into());
        }
        self.record = Record::prepared(d.identity, self.device_id, self.owner)?;
        let upload_bytes = plan.upload_bytes();
        let staging_bytes = plan.staging_bytes();
        let source_bytes = plan.source_bytes();
        let prepared = autoreleasepool(|_| {
            let bytes = plan.stage(source.pixels())?;
            let staging = upload(&renderer.device, &bytes)?;
            if staging.length() < staging_bytes {
                return Err("Metal staging buffer shorter than requested".into());
            }
            // Standard RETAINING command buffer. Do not substitute the unretained
            // variant: after Material::drop these native refs are the GPU owners.
            let command = renderer
                .queue
                .commandBuffer()
                .ok_or("async upload command allocation failed")?;
            let blit = command
                .blitCommandEncoder()
                .ok_or("async upload encoder allocation failed")?;
            let texture = self.texture.as_ref().unwrap().clone();
            for r in plan.regions() {
                // SAFETY: identical immutable checked plan/copy API as G05-A;
                // initialized staging and complete target ranges on the same device.
                unsafe {
                    blit.copyFromBuffer_sourceOffset_sourceBytesPerRow_sourceBytesPerImage_sourceSize_toTexture_destinationSlice_destinationLevel_destinationOrigin(
                    &staging, r.staging, r.row, r.image, MTLSize {width:r.width,height:r.width,depth:1}, &texture,r.slice,r.level,MTLOrigin{x:0,y:0,z:0});
                }
            }
            blit.endEncoding();
            self.record.advance(Phase::Prepared, Phase::Encoded)?;
            permit.account(plan.texture_bytes(), staging_bytes, source_bytes);
            let lease = Arc::new(KeepAlive {
                owners: Ownership {
                    source: Arc::new(source),
                    previous: self.source.take(),
                    _permit: permit,
                },
                _device: renderer.device.clone(),
                _queue: renderer.queue.clone(),
            });
            let retained = lease.clone();
            let keepalive = move |_command: NonNull<ProtocolObject<dyn MTLCommandBuffer>>| {
                // Retain all CPU owners/permit until this terminal callback ends.
                // No pointer dereference, no self access, no completion assertion.
                let _ = &retained;
            };
            fn require_send_sync_static<T: Send + Sync + 'static>(_: &T) {}
            require_send_sync_static(&keepalive);
            let block = RcBlock::new(keepalive);
            // SAFETY: addCompletedHandler COPIES the owned block before return.
            // Capture is Send+Sync+'static (compiler checked), contains owned CPU
            // data and explicitly thread-safe device/queue refs, no borrowed stack
            // data or command reference cycle. The callback never marks success.
            unsafe {
                command.addCompletedHandler(RcBlock::as_ptr(&block));
            }
            Ok::<_, String>(Pending {
                command,
                _staging: staging,
                _texture: texture,
                lease,
                description: d,
                revoked: None,
            })
        });
        let pending = match prepared {
            Ok(pending) => pending,
            Err(error) => {
                self.record.fail();
                self.source = None;
                self.counters.source_retained = 0;
                self.counters.failed_operations = self.counters.failed_operations.saturating_add(1);
                if error.contains("allocation failed") {
                    self.counters.failed_allocations =
                        self.counters.failed_allocations.saturating_add(1);
                }
                return Err(error);
            }
        };
        self.pending = Some(pending); // ownership installed BEFORE committing
        self.counters.pending = 1;
        self.counters.staging_capacity = staging_bytes;
        self.counters.source_retained = source_bytes;
        self.counters.staging_peak = self.counters.staging_peak.max(staging_bytes);
        self.counters.source_peak = self.counters.source_peak.max(source_bytes);
        self.pending.as_ref().unwrap().command.commit();
        self.counters.submitted_operations = self.counters.submitted_operations.saturating_add(1);
        self.counters.upload_bytes = self
            .counters
            .upload_bytes
            .saturating_add(upload_bytes as u64);
        // Even a protocol error here retains the submitted operation until actual
        // terminal observation/callback cleanup; never discard it on an Err path.
        self.record
            .advance(Phase::Encoded, Phase::Submitted)
            .inspect_err(|_| {
                self.pending.as_mut().unwrap().revoked =
                    Some("submission protocol invalidated".into());
                self.record.invalidate();
            })
    }

    /// One actual status/error observation, no waits, sleeps, spin or callback
    /// synchronization. Tiny uploads may already be terminal on the first poll.
    pub fn poll_upload(
        &mut self,
        renderer: &TerrainRenderer,
        authority: &S::Authority,
    ) -> Poll<S::Generation> {
        let Some(pending) = self.pending.as_mut() else {
            return Poll::Idle;
        };
        self.counters.pending_polls = self.counters.pending_polls.saturating_add(1);
        if pending.revoked.is_none() {
            let invalid = if self.texture.is_none() {
                Some("material released during pending upload".into())
            } else if self.device_id != renderer.upload_device {
                Some("wrong/recreated device owner".into())
            } else if pending.lease.owners.source.description() != pending.description {
                Some("pending source metadata changed".into())
            } else {
                pending.lease.owners.source.check_current(authority).err()
            };
            if let Some(reason) = invalid {
                pending.revoked = Some(reason);
                if self.record.phase() != Phase::Invalidated {
                    self.counters.invalidations = self.counters.invalidations.saturating_add(1);
                }
                self.record.invalidate();
            }
        }
        let status = pending.command.status();
        // NSError alone is NOT a terminal lifetime signal. Do not free resources
        // until Metal reports Completed/Error, even for invalidated source/owner.
        if status != MTLCommandBufferStatus::Completed && status != MTLCommandBufferStatus::Error {
            if pending.revoked.is_none() && self.record.phase() != Phase::Invalidated {
                let _ = self.record.observe(None);
            }
            return Poll::Pending;
        }
        let error = pending.command.error();
        let pending = self.pending.take().unwrap();
        self.counters.pending = 0;
        self.counters.staging_capacity = 0;
        let result = if let Some(reason) = pending.revoked.as_ref() {
            self.record.invalidate();
            Err(reason.clone())
        } else if self.record.phase() == Phase::Invalidated {
            Err("pending upload invalidated/released".into())
        } else if status != MTLCommandBufferStatus::Completed || error.is_some() {
            let _ = self.record.observe(Some(false));
            Err(format!("async Metal upload failed: {status:?}, {error:?}"))
        } else {
            self.record
                .observe(Some(true))
                .and_then(|t| t.ok_or_else(|| "terminal command produced no receipt".into()))
        };
        match result {
            Ok(ticket) => {
                if pending.lease.owners.previous.is_some() {
                    self.counters.reused_resources =
                        self.counters.reused_resources.saturating_add(1);
                }
                self.source = Some(pending.lease.owners.source.clone());
                self.description = pending.description;
                self.counters.source_retained = self.source.as_ref().unwrap().retained_bytes();
                self.counters.completed_uploads = self.counters.completed_uploads.saturating_add(1);
                Poll::Completed(ticket)
            }
            Err(error) => {
                self.source = None;
                self.counters.source_retained = 0;
                if self.texture.is_none() {
                    self.counters.texture_payload = 0;
                }
                self.counters.failed_operations = self.counters.failed_operations.saturating_add(1);
                Poll::Failed(error)
            }
        }
        // Pending (command/staging/texture/lease refs) drops here WITHOUT waiting.
        // The Metal-copied lease outlives this if its callback hasn't retired yet.
        // It keeps the global slot busy until the final cleanup ref disappears.
    }
}
