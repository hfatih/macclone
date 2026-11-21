import { invoke } from '@tauri-apps/api/core'

export interface Disk {
  number: number
  model: string
  serial: string | null
  id: string | null
  bytes: number
  logical: number
  physical: number
  bus: string
  boot: boolean | null
  system: boolean | null
  offline: boolean | null
  readonly: boolean | null
  clustered: boolean | null
  health: string | null
}

export type Input = { kind: 'image'; path: string; sector: number } | { kind: 'disk'; number: number; id: string }

export interface Layout {
  table: string
  filesystem: string
  warnings: string[]
  partitions: { start_lba: number; end_lba: number; kind: string; label: string; filesystem: string }[]
}

export interface ClonePlan {
  blockers: string[]
  gpt_fit: { state: 'fits'; target_bytes: number; minimum_bytes: number } | { state: 'blocked'; reason: string } | null
}

export type Outcome = 'idle' | 'running' | 'completed' | 'partial' | 'cancelled' | 'failed'

export interface Status {
  id: number
  revision: number
  running: boolean
  outcome: Outcome
  operation: 'image' | 'recovery' | 'verify' | null
  progress: { phase: string; bytes: number; total: number; missing_sectors: number }
  message: string | null
  report: { outcome: 'verified' | 'partial'; sha256: string; verified_bytes: number; missing_sectors: number } | null
}

export interface ApfsVolumeInfo {
  index: number
  name: string
  role: string
  used_bytes: number
  encrypted: boolean
}

export interface ApfsEntry {
  name: string
  path: string
  kind: 'dir' | 'file' | 'symlink' | 'other'
  size: number
  inode: number
}

export interface ExtractSummary {
  files: number
  dirs: number
  bytes: number
  skipped: number
}

export const api = {
  disks: () => invoke<Disk[]>('list_disks'),
  inspect: (input: Input) => invoke<Layout>('inspect_disk', { input }),
  apfsVolumes: (input: Input, startLba: number, endLba: number) =>
    invoke<ApfsVolumeInfo[]>('apfs_volumes', { input, startLba, endLba }),
  apfsListDir: (input: Input, startLba: number, endLba: number, volumeIndex: number, path: string) =>
    invoke<ApfsEntry[]>('apfs_list_dir', { input, startLba, endLba, volumeIndex, path }),
  apfsExtract: (input: Input, startLba: number, endLba: number, volumeIndex: number, path: string, destination: string) =>
    invoke<ExtractSummary>('apfs_extract', { input, startLba, endLba, volumeIndex, path, destination }),
  plan: (source: Disk, target: Disk) => invoke<ClonePlan>('plan_clone', {
    source: source.number,
    target: target.number,
    sourceId: source.id,
    targetId: target.id,
  }),
  status: () => invoke<Status>('get_operation_status'),
  image: (input: Input, directory: string, block: number, recovery: boolean, resume: boolean, targetBytes?: number) =>
    invoke<number>('create_image', { request: { input, directory, options: { block, recovery }, resume, target_bytes: targetBytes } }),
  verify: (input: Input, expected: string) => invoke<number>('verify_image', { input, expected }),
  cancel: (id: number) => invoke<void>('cancel_operation', { id }),
  setDiskOffline: (disk: Disk, offline: boolean) =>
    invoke<void>('set_disk_offline', { number: disk.number, id: disk.id, offline }),
  prepareTargetDisk: (disk: Disk) =>
    invoke<void>('prepare_target_disk', { number: disk.number, id: disk.id }),
}
