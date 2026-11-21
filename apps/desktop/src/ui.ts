import type { Disk, Outcome } from './api'

const numberFormat = new Intl.NumberFormat('tr-TR', { maximumFractionDigits: 1 })
const sizes = ['B', 'KiB', 'MiB', 'GiB', 'TiB', 'PiB']

export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return '—'
  let unit = 0
  while (bytes >= 1024 && unit < sizes.length - 1) {
    bytes /= 1024
    unit++
  }
  return numberFormat.format(bytes) + ' ' + sizes[unit]
}

export const outcomeLabel: Record<Outcome, string> = {
  idle: 'Beklemede',
  running: 'Çalışıyor',
  completed: 'Tamamlandı',
  partial: 'Eksik sektör var',
  cancelled: 'Durduruldu',
  failed: 'Başarısız',
}

export function diskLabel(disk: Disk): string {
  return `Disk ${disk.number} · ${disk.model || 'Model bilinmiyor'} · ${formatBytes(disk.bytes)}`
}
