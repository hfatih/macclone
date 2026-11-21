<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { listen } from '@tauri-apps/api/event'
import {
  Activity, ArrowLeftRight, ArrowRight, ArrowUpRight, CircleAlert, CircleCheck,
  ClipboardList, Database, Download, FileArchive, Folder, HardDrive, LayoutDashboard, LifeBuoy,
  RefreshCw, ScanSearch, Settings2, ShieldCheck, Trash2,
} from '@lucide/vue'
import logo from '../src-tauri/icons/icon.png'
import pkg from '../package.json'
import { api, type ApfsEntry, type ApfsVolumeInfo, type ClonePlan, type Disk, type Input, type Layout, type Status } from './api'
import { diskLabel, formatBytes, outcomeLabel } from './ui'
import DeviceTable from './DeviceTable.vue'
import OperationPanel from './OperationPanel.vue'

const navigation = [
  { label: 'Çalışma alanı', items: [
    { id: 'overview', title: 'Genel bakış', icon: LayoutDashboard },
    { id: 'disks', title: 'Diskler', icon: HardDrive },
  ] },
  { label: 'Disk işlemleri', items: [
    { id: 'plan', title: 'Klon planı', icon: ArrowLeftRight },
    { id: 'image', title: 'Disk imajı', icon: FileArchive },
    { id: 'recovery', title: 'Kurtarma', icon: LifeBuoy },
    { id: 'inspect', title: 'Dosya sistemi', icon: ScanSearch },
    { id: 'verify', title: 'Doğrulama', icon: ShieldCheck },
  ] },
  { label: 'Uygulama', items: [
    { id: 'logs', title: 'Günlük', icon: ClipboardList },
    { id: 'settings', title: 'Ayarlar', icon: Settings2 },
  ] },
] as const
type Page = typeof navigation[number]['items'][number]['id']

const descriptions: Record<Page, string> = {
  overview: 'Bağlı diskleri, kopyalama planlarını ve son işlemi tek yerden takip edin.',
  disks: 'Aygıt kimliğini, bağlantı türünü ve sektör boyutlarını kontrol edin.',
  plan: 'HDD ve SSD arasında kopyalamadan önce iki diskin uyumluluğunu denetleyin.',
  image: 'Bir diski veya RAW dosyasını yeni bir çalışma klasörüne kopyalayın.',
  recovery: 'Sorunlu kaynaktan RAW imaj alın ve okunamayan sektörleri kaydedin.',
  inspect: 'GPT, MBR ve bölüm imzalarını kaynağa yazmadan inceleyin.',
  verify: 'Kaynağı yeniden okuyup SHA-256 özetini beklenen değerle karşılaştırın.',
  logs: 'Bu oturumda oluşan işlem sonuçlarını ve hataları inceleyin.',
  settings: 'RAW dosya geometrisini ve kopyalama blok boyutunu ayarlayın.',
}

const page = ref<Page>('overview')
const heading = ref<HTMLElement>()
const disks = ref<Disk[]>([])
const inventoryError = ref('')
const inventoryLoading = ref(true)
const statusError = ref('')
const cancelError = ref('')
const actionError = ref('')
const action = ref<'inspect' | 'plan' | 'image' | 'verify' | 'disk' | 'apfs' | null>(null)
const activity = ref<{ timestamp: string; message: string }[]>([])
const apfsPartition = ref<{ start_lba: number; end_lba: number } | null>(null)
const apfsVolumes = ref<ApfsVolumeInfo[]>([])
const apfsVolume = ref<ApfsVolumeInfo | null>(null)
const apfsPath = ref('/')
const apfsEntries = ref<ApfsEntry[]>([])
const apfsDest = ref('')
const apfsNotice = ref('')
const mode = ref<'image' | 'disk'>('image')
const path = ref('')
const directory = ref('')
const expected = ref('')
const selected = ref<number | null>(null)
const target = ref<number | null>(null)
const resume = ref(false)
const sector = ref(512)
const block = ref(1048576)
const layout = ref<Layout | null>(null)
const planResult = ref<ClonePlan | null>(null)
const imagePreset = ref<{ source: Disk; target: Disk } | null>(null)
const status = ref<Status | null>(null)
const startingId = ref<number | null>(null)
const stoppingId = ref<number | null>(null)
const speed = ref(0)

let selectionRevision = 0
let inventoryRevision = 0
let pendingStatus = false
let disposed = false
let unlisten: (() => void) | undefined
let timer: ReturnType<typeof setInterval> | undefined
let sample: { id: number; phase: string; bytes: number; time: number } | undefined

type NavItem = typeof navigation[number]['items'][number]
const navItems = navigation.flatMap<NavItem>(group => group.items)
const title = computed(() => navItems.find(item => item.id === page.value)?.title)
const sourceDisk = computed(() => disks.value.find(disk => disk.number === selected.value))
const targetDisk = computed(() => disks.value.find(disk => disk.number === target.value))
const totalCapacity = computed(() => disks.value.reduce((total, disk) => total + disk.bytes, 0))
const overview = computed(() => page.value === 'overview' || page.value === 'disks')
const imagePage = computed(() => page.value === 'image' || page.value === 'recovery')
const sourcePage = computed(() => imagePage.value || page.value === 'inspect' || page.value === 'verify')
const canRefresh = computed(() => overview.value || page.value === 'plan' || (sourcePage.value && mode.value === 'disk'))
const busy = computed(() => Boolean(action.value || status.value?.running || startingId.value !== null))
const stopping = computed(() => stoppingId.value !== null && status.value?.id === stoppingId.value)
const validDigest = computed(() => /^[a-f0-9]{64}$/i.test(expected.value.trim()))
const invalidDigest = computed(() => Boolean(expected.value.trim()) && !validDigest.value)
const gptFit = computed(() => planResult.value?.gpt_fit)
const canUseFit = computed(() => !busy.value && !inventoryLoading.value
  && gptFit.value?.state === 'fits' && gptFit.value.target_bytes === targetDisk.value?.bytes)
const presetValid = computed(() => {
  const preset = imagePreset.value
  return !preset || (mode.value === 'disk'
    && hasSameGeometry(preset.source, sourceDisk.value)
    && hasSameGeometry(preset.target, targetDisk.value))
})
const subject = computed<Input | null>(() => {
  if (mode.value === 'image') {
    return path.value.trim() ? { kind: 'image', path: path.value.trim(), sector: sector.value } : null
  }
  const disk = sourceDisk.value
  return disk?.id ? { kind: 'disk', number: disk.number, id: disk.id } : null
})
const canStart = computed(() => {
  if (!subject.value || busy.value || (mode.value === 'disk' && inventoryLoading.value)) return false
  if (imagePage.value) {
    return Boolean(directory.value.trim()) && presetValid.value && (mode.value !== 'disk' || sourceDisk.value?.offline === true)
  }
  if (page.value === 'verify') {
    return validDigest.value && (mode.value !== 'disk' || sourceDisk.value?.offline === true)
  }
  return page.value === 'inspect'
})
const planHint = computed(() => {
  const a = sourceDisk.value
  const b = targetDisk.value
  if (!a || !b) return null
  if (a.number === b.number) return { tone: 'warning', message: 'Kaynak ve hedef için farklı diskler seçin.' }
  if (a.logical !== b.logical) return { tone: 'warning', message: 'Disklerin mantıksal sektör boyutları farklı.' }
  if (a.bytes > b.bytes) return { tone: 'neutral', message: 'Plan denetimi, mevcut GPT bölümlerinin daha küçük hedefe sığıp sığmadığını ölçer.' }
  if (a.bytes < b.bytes) return { tone: 'neutral', message: 'Büyük hedef için sağlam bir GPT ve ayrı bir GPT taşıma onayı gerekir.' }
  return { tone: 'neutral', message: 'Eşit kapasitede tam disk kopyası planlanır.' }
})
const canPlan = computed(() =>
  !busy.value && !inventoryLoading.value && Boolean(sourceDisk.value?.id && targetDisk.value?.id)
  && sourceDisk.value?.number !== targetDisk.value?.number,
)
const canPrepareTarget = computed(() => {
  const disk = targetDisk.value
  return !busy.value && !inventoryLoading.value && Boolean(disk?.id)
    && disk?.boot === false && disk.system === false && disk.clustered === false && disk.readonly !== true
    && disk.number !== sourceDisk.value?.number
})
const startLabel = computed(() => {
  if (action.value === 'inspect') return 'İnceleniyor…'
  if (action.value === 'verify' || action.value === 'image' || startingId.value !== null) return 'Hazırlanıyor…'
  if (page.value === 'verify') return 'İçeriği doğrula'
  if (page.value === 'inspect') return 'Bölüm tablosunu incele'
  return resume.value ? 'İşe devam et' : 'İmaj al'
})

async function navigate(destination: Page) {
  if (page.value === destination) return
  if (page.value === 'image') imagePreset.value = null
  page.value = destination
  await nextTick()
  heading.value?.focus()
}

function log(message: string) {
  const entry = { timestamp: new Date().toLocaleTimeString('tr-TR'), message }
  activity.value = [entry, ...activity.value].slice(0, 80)
}

function apply(next: Status) {
  const previous = status.value
  if (previous && (next.id < previous.id || (next.id === previous.id && next.revision <= previous.revision))) return
  const now = performance.now()
  const progress = next.progress
  if (sample?.id !== next.id || sample.phase !== progress.phase) {
    speed.value = 0
    sample = { id: next.id, phase: progress.phase, bytes: progress.bytes, time: now }
  } else if (progress.bytes > sample.bytes && now > sample.time) {
    speed.value = (progress.bytes - sample.bytes) * 1000 / (now - sample.time)
    sample = { id: next.id, phase: progress.phase, bytes: progress.bytes, time: now }
  }
  if (stoppingId.value !== null && (next.id !== stoppingId.value || !next.running)) stoppingId.value = null
  if (startingId.value !== null && next.id >= startingId.value) startingId.value = null
  if (next.id !== previous?.id || !next.running) cancelError.value = ''
  if (next.message && (previous?.message !== next.message || previous.id !== next.id)) log(next.message)
  status.value = next
}

function isSameDisk(previous: Disk | undefined, next: Disk | undefined) {
  return Boolean(previous?.id && next && previous.id === next.id && previous.serial === next.serial)
}

function hasSameGeometry(previous: Disk, next: Disk | undefined) {
  return next !== undefined && isSameDisk(previous, next) && previous.bytes === next.bytes && previous.logical === next.logical
}

async function refresh() {
  const run = ++inventoryRevision
  inventoryLoading.value = true
  inventoryError.value = ''
  try {
    const found = await api.disks()
    if (run !== inventoryRevision || disposed) return
    if (selected.value !== null && !isSameDisk(sourceDisk.value, found.find(d => d.number === selected.value))) selected.value = null
    if (target.value !== null && !isSameDisk(targetDisk.value, found.find(d => d.number === target.value))) target.value = null
    disks.value = found
  } catch (error) {
    if (run !== inventoryRevision || disposed) return
    inventoryError.value = String(error)
    selected.value = null
    target.value = null
    disks.value = []
  } finally {
    if (run === inventoryRevision && !disposed) inventoryLoading.value = false
  }
}

async function act(kind: NonNullable<typeof action.value>, request: () => Promise<void>) {
  if (action.value) return
  const revision = selectionRevision
  actionError.value = ''
  action.value = kind
  try {
    await request()
  } catch (error) {
    if (revision === selectionRevision) actionError.value = String(error)
    log(String(error))
  } finally {
    action.value = null
  }
}

async function start() {
  const input = subject.value
  if (!input || !canStart.value) return
  const current = page.value
  const revision = selectionRevision
  if (current === 'inspect') {
    layout.value = null
    await act('inspect', async () => {
      const result = await api.inspect(input)
      if (revision === selectionRevision) layout.value = result
    })
    return
  }
  await act(current === 'verify' ? 'verify' : 'image', async () => {
    const id = current === 'verify'
      ? await api.verify(input, expected.value.trim())
      : await api.image(input, directory.value.trim(), block.value, current === 'recovery', resume.value, imagePreset.value?.target.bytes)
    if ((status.value?.id ?? 0) < id) startingId.value = id
    await pollStatus()
  })
}

async function checkPlan() {
  const source = sourceDisk.value
  const destination = targetDisk.value
  if (!source || !destination || !canPlan.value) return
  const revision = selectionRevision
  planResult.value = null
  await act('plan', async () => {
    const result = await api.plan(source, destination)
    if (revision === selectionRevision) planResult.value = result
  })
}

async function openSizedImage() {
  const source = sourceDisk.value
  const destination = targetDisk.value
  if (!source || !destination || !canUseFit.value) return
  mode.value = 'disk'
  directory.value = ''
  resume.value = false
  imagePreset.value = { source, target: destination }
  await navigate('image')
}

function reverseDirection() {
  const source = selected.value
  selected.value = target.value
  target.value = source
}

function localizeBlocker(reason: string): string {
  const offline = /^Disk (\d+) must be offline, non-system, non-boot and non-clustered$/.exec(reason)
  if (offline) return `Disk ${offline[1]} çevrimdışı olmalı (sistem, önyükleme veya küme diski olmamalı).`
  const map: Record<string, string> = {
    'Destination is smaller than the source disk': 'Hedef disk kaynak diskten daha küçük.',
    'Logical sector sizes differ': 'Disklerin mantıksal sektör boyutları farklı.',
    'Same or ambiguous physical identity': 'Kaynak ve hedef aynı disk veya fiziksel kimlik belirsiz.',
    'Target is read-only or write state is unknown': 'Hedef disk salt okunur veya yazma durumu bilinmiyor.',
  }
  return map[reason] ?? reason
}

function localizeGptReason(reason: string): string {
  const minBytes = /^GPT partitions and backup metadata need at least (\d+) bytes at their current positions$/.exec(reason)
  if (minBytes) {
    return `Kaynak diskteki bölümler mevcut boyutlarıyla en az ${formatBytes(Number(minBytes[1]))} gerektiriyor (hedef diskten büyük). İçindeki veri az olsa bile APFS bölümü Windows'tan güvenli küçültülemez; Mac'te "diskutil apfs resizeContainer diskXs2 220g" ile küçültüp tekrar deneyin.`
  }
  const map: Record<string, string> = {
    'Protective MBR does not describe the entire source disk': 'Koruyucu MBR tüm kaynak diski kapsamıyor.',
    'Source GPT copies or metadata placement disagree': 'Kaynak diskteki birincil ve yedek GPT tabloları uyuşmuyor.',
    'Expected a single protective MBR entry': 'Kaynak diskte birden fazla MBR girişi (hibrit MBR) var.',
  }
  return map[reason] ?? reason
}

async function toggleDiskOffline(disk: Disk) {
  if (busy.value || !disk.id || disk.offline === null) return
  const nextOffline = !disk.offline
  await act('disk', async () => {
    await api.setDiskOffline(disk, nextOffline)
    log(`Disk ${disk.number} ${nextOffline ? 'çevrimdışı' : 'çevrimiçi'} yapıldı.`)
    await refresh()
  })
}

async function prepareTarget() {
  const destination = targetDisk.value
  if (!destination || !canPrepareTarget.value) return
  const ok = window.confirm(
    `DİKKAT: Disk ${destination.number} (${destination.model || 'Model bilinmiyor'} · ${formatBytes(destination.bytes)} · Seri: ${destination.serial || 'bilinmiyor'}) üzerindeki TÜM bölümler ve veriler kalıcı olarak silinecek, disk sıfırlanıp çevrimdışı yapılacak.\n\nDevam etmek istiyor musunuz?`,
  )
  if (!ok) return
  await act('disk', async () => {
    await api.prepareTargetDisk(destination)
    log(`Disk ${destination.number} sıfırlandı ve çevrimdışı yapıldı.`)
    await refresh()
  })
  if (sourceDisk.value && targetDisk.value && canPlan.value) await checkPlan()
}

async function inspectDisk(disk: Disk) {
  if (busy.value) return
  selected.value = disk.number
  mode.value = 'disk'
  await navigate('inspect')
  await start()
}

async function openApfsContainer(startLba: number, endLba: number) {
  const input = subject.value
  if (!input || busy.value) return
  apfsPartition.value = { start_lba: startLba, end_lba: endLba }
  apfsVolumes.value = []
  apfsVolume.value = null
  apfsPath.value = '/'
  apfsEntries.value = []
  apfsNotice.value = ''
  await act('apfs', async () => {
    apfsVolumes.value = await api.apfsVolumes(input, startLba, endLba)
    log(`APFS konteyneri açıldı (${apfsVolumes.value.length} hacim).`)
  })
}

async function browseApfs(vol: ApfsVolumeInfo, nextPath = '/') {
  const input = subject.value
  const part = apfsPartition.value
  if (!input || !part || busy.value) return
  apfsVolume.value = vol
  apfsPath.value = nextPath
  apfsEntries.value = []
  apfsNotice.value = ''
  await act('apfs', async () => {
    apfsEntries.value = await api.apfsListDir(input, part.start_lba, part.end_lba, vol.index, nextPath)
  })
}

async function apfsGoUp() {
  if (!apfsVolume.value || apfsPath.value === '/') return
  const parts = apfsPath.value.split('/').filter(Boolean)
  parts.pop()
  await browseApfs(apfsVolume.value, parts.length ? `/${parts.join('/')}` : '/')
}

async function extractApfs(targetPath: string) {
  const input = subject.value
  const part = apfsPartition.value
  const vol = apfsVolume.value
  const dest = apfsDest.value.trim()
  if (!input || !part || !vol || !dest || busy.value) return
  apfsNotice.value = ''
  await act('apfs', async () => {
    const res = await api.apfsExtract(input, part.start_lba, part.end_lba, vol.index, targetPath, dest)
    const msg = `Kopyalandı: ${res.files} dosya, ${res.dirs} klasör (${formatBytes(res.bytes)})${res.skipped ? ` · ${res.skipped} atlandı` : ''} → ${dest}`
    apfsNotice.value = msg
    log(msg)
  })
}

async function cancelOperation() {
  const current = status.value
  if (!current?.running || stopping.value) return
  stoppingId.value = current.id
  cancelError.value = ''
  try {
    await api.cancel(current.id)
  } catch (error) {
    await pollStatus()
    if (status.value?.id === current.id && status.value.running) cancelError.value = String(error)
    if (stoppingId.value === current.id) stoppingId.value = null
  }
}

watch([page, selected, target, path, sector, mode, disks, inventoryLoading], () => {
  selectionRevision++
  actionError.value = ''
  layout.value = null
  planResult.value = null
  apfsPartition.value = null
  apfsVolumes.value = []
  apfsVolume.value = null
  apfsEntries.value = []
  apfsNotice.value = ''
}, { flush: 'sync' })

async function pollStatus() {
  if (pendingStatus || disposed) return
  pendingStatus = true
  try {
    const latest = await api.status()
    if (disposed) return
    statusError.value = ''
    apply(latest)
  } catch (error) {
    if (!disposed) statusError.value = String(error)
  } finally {
    pendingStatus = false
  }
}

onMounted(() => {
  timer = setInterval(() => { void pollStatus() }, 1500)
  void listen<Status>('operation', ({ payload }) => {
    if (disposed) return
    statusError.value = ''
    apply(payload)
  }).then(remove => {
    if (disposed) remove()
    else unlisten = remove
  }).catch(error => {
    if (!disposed) log(String(error))
  })
  void pollStatus()
  void refresh()
})

onUnmounted(() => {
  disposed = true
  unlisten?.()
  clearInterval(timer)
})
</script>

<template>
  <div class="shell">
    <a href="#main-content" class="skip-link">İçeriğe geç</a>
    <aside class="sidebar">
      <div class="brand">
        <img :src="logo" width="44" height="44" alt="">
        <div><strong>MacClone</strong><span>Starlit Studios · v{{ pkg.version }}</span></div>
      </div>
      <nav class="navigation" aria-label="Ana menü">
        <div v-for="group in navigation" :key="group.label" class="nav-group">
          <p class="nav-label">{{ group.label }}</p>
          <button
            v-for="item in group.items" :key="item.id" type="button"
            class="nav-item" :aria-current="page === item.id ? 'page' : undefined"
            @click="navigate(item.id)"
          >
            <component :is="item.icon" :size="17" :stroke-width="1.8" aria-hidden="true" />
            <span>{{ item.title }}</span>
          </button>
        </div>
      </nav>
    </aside>

    <main id="main-content" class="workspace">
      <header class="page-header">
        <div>
          <h1 ref="heading" tabindex="-1">{{ title }}</h1>
          <p>{{ descriptions[page] }}</p>
        </div>
        <button v-if="canRefresh" type="button" class="button button-quiet" :disabled="inventoryLoading" @click="refresh">
          <RefreshCw :size="16" :class="{ spinning: inventoryLoading }" aria-hidden="true" />
          {{ inventoryLoading ? 'Diskler okunuyor…' : 'Diskleri yenile' }}
        </button>
      </header>

      <div v-if="actionError" class="notice notice-danger" role="alert">
        <CircleAlert :size="17" aria-hidden="true" /><span>{{ actionError }}</span>
      </div>

      <template v-if="overview">
        <div v-if="page === 'overview'" class="summary">
          <div class="panel stat">
            <span class="stat-icon"><HardDrive :size="19" aria-hidden="true" /></span>
            <dl><dt>Fiziksel diskler</dt><dd>{{ inventoryLoading || inventoryError ? '—' : disks.length }}</dd></dl>
          </div>
          <div class="panel stat">
            <span class="stat-icon"><Database :size="19" aria-hidden="true" /></span>
            <dl><dt>Toplam kapasite</dt><dd>{{ inventoryLoading || inventoryError ? '—' : formatBytes(totalCapacity) }}</dd></dl>
          </div>
          <div class="panel stat">
            <span class="stat-icon"><Activity :size="19" aria-hidden="true" /></span>
            <dl><dt>İşlem durumu</dt><dd>{{ startingId !== null ? 'Hazırlanıyor' : status ? outcomeLabel[status.outcome] : '—' }}</dd></dl>
          </div>
        </div>

        <section class="panel" aria-labelledby="disks-title" :aria-busy="inventoryLoading">
          <div class="panel-heading">
            <div class="panel-title"><HardDrive :size="19" aria-hidden="true" /><h2 id="disks-title">Disk envanteri</h2></div>
            <span v-if="!inventoryLoading && !inventoryError" class="quiet">{{ disks.length }} aygıt</span>
          </div>
          <div v-if="inventoryError" class="panel-body">
            <div class="notice notice-danger" role="alert">
              <CircleAlert :size="18" aria-hidden="true" />
              <div><strong>Disk listesi alınamadı.</strong><p>{{ inventoryError }}</p></div>
            </div>
          </div>
          <p v-else-if="inventoryLoading" class="empty-state" role="status">Disk envanteri okunuyor…</p>
          <DeviceTable v-else-if="disks.length" :disks="disks" :busy="busy" @inspect="inspectDisk" @toggle-offline="toggleDiskOffline" />
          <div v-else class="empty-state">
            <HardDrive :size="24" aria-hidden="true" />
            <strong>Bağlı disk bulunamadı</strong>
            <p>Bir disk bağladıktan sonra envanteri yenileyin.</p>
          </div>
        </section>

        <section v-if="page === 'overview'" aria-labelledby="actions-title">
          <h2 id="actions-title" class="section-title">Hızlı işlemler</h2>
          <div class="quick-actions">
            <button class="panel quick-action" type="button" @click="navigate('plan')">
              <span class="stat-icon"><ArrowLeftRight :size="20" aria-hidden="true" /></span>
              <span><strong>Klon planı oluştur</strong><small>Kaynak ve hedef diskleri karşılaştır.</small></span>
              <ArrowUpRight :size="17" aria-hidden="true" />
            </button>
            <button class="panel quick-action" type="button" @click="navigate('image')">
              <span class="stat-icon"><FileArchive :size="20" aria-hidden="true" /></span>
              <span><strong>RAW imaj al</strong><small>Yeni bir dosya tabanlı kopya oluştur.</small></span>
              <ArrowUpRight :size="17" aria-hidden="true" />
            </button>
          </div>
        </section>
      </template>

      <section v-if="page === 'plan'" class="panel" aria-labelledby="plan-title">
        <div class="panel-heading">
          <div class="panel-title"><ArrowLeftRight :size="19" aria-hidden="true" /><h2 id="plan-title">Kaynak ve hedef</h2></div>
          <span class="pill pill-neutral">Salt okunur plan</span>
        </div>
        <form class="panel-body form" @submit.prevent="checkPlan">
          <div v-if="inventoryError" class="notice notice-danger"><CircleAlert :size="17" aria-hidden="true" /><span>{{ inventoryError }}</span></div>
          <div class="field-grid">
            <div class="field">
              <label for="plan-source">Kaynak disk</label>
              <select id="plan-source" v-model="selected" :disabled="inventoryLoading" :aria-describedby="sourceDisk ? 'plan-source-details' : undefined">
                <option :value="null">{{ inventoryLoading ? 'Diskler okunuyor…' : 'Kaynak seçin' }}</option>
                <option v-for="disk in disks" :key="disk.number" :value="disk.number" :disabled="!disk.id">
                  {{ diskLabel(disk) }}
                </option>
              </select>
              <span v-if="sourceDisk" id="plan-source-details" class="field-meta">Seri {{ sourceDisk.serial || 'bilinmiyor' }} · {{ sourceDisk.logical }} B sektör</span>
            </div>
            <div class="field">
              <label for="plan-target">Hedef disk</label>
              <select id="plan-target" v-model="target" :disabled="inventoryLoading" :aria-describedby="targetDisk ? 'plan-target-details' : undefined">
                <option :value="null">{{ inventoryLoading ? 'Diskler okunuyor…' : 'Hedef seçin' }}</option>
                <option v-for="disk in disks" :key="disk.number" :value="disk.number" :disabled="!disk.id">
                  {{ diskLabel(disk) }}
                </option>
              </select>
              <span v-if="targetDisk" id="plan-target-details" class="field-meta">Seri {{ targetDisk.serial || 'bilinmiyor' }} · {{ targetDisk.logical }} B sektör</span>
            </div>
          </div>
          <div v-if="planHint" class="notice" :class="`notice-${planHint.tone}`">
            <CircleAlert :size="17" aria-hidden="true" /><span>{{ planHint.message }}</span>
          </div>
          <div class="actions">
            <button class="button button-primary" type="submit" :disabled="!canPlan">
              <ShieldCheck :size="17" aria-hidden="true" /> {{ action === 'plan' ? 'Denetleniyor…' : 'Planı denetle' }}
            </button>
            <button class="button button-quiet" type="button" :disabled="busy || !sourceDisk || !targetDisk || selected === target" @click="reverseDirection">
              <ArrowLeftRight :size="17" aria-hidden="true" /> Kaynak ve hedefi değiştir
            </button>
            <button class="button button-quiet" type="button" :disabled="!canPrepareTarget" @click="prepareTarget">
              <Trash2 :size="17" aria-hidden="true" /> {{ action === 'disk' ? 'Sıfırlanıyor…' : 'Hedef diski sıfırla ve çevrimdışı yap' }}
            </button>
          </div>
          <div v-if="planResult" class="notice" :class="planResult.blockers.length ? 'notice-warning' : 'notice-success'" role="status">
            <CircleAlert v-if="planResult.blockers.length" :size="18" aria-hidden="true" /><CircleCheck v-else :size="18" aria-hidden="true" />
            <div>
              <strong>{{ planResult.blockers.length ? 'Fiziksel klon planı engellendi' : 'Aygıt uyumluluğu denetimi geçti' }}</strong>
              <ul v-if="planResult.blockers.length"><li v-for="reason in planResult.blockers" :key="reason">{{ localizeBlocker(reason) }}</li></ul>
            </div>
          </div>
          <div v-if="gptFit" class="notice" :class="gptFit.state === 'fits' ? 'notice-success' : 'notice-danger'" role="status">
            <CircleCheck v-if="gptFit.state === 'fits'" :size="18" aria-hidden="true" />
            <CircleAlert v-else :size="18" aria-hidden="true" />
            <div v-if="gptFit.state === 'fits'">
              <strong>GPT bölümleri hedef boyutuna sığıyor</strong>
              <p>Hedef {{ formatBytes(gptFit.target_bytes) }} · Gerekli en az {{ formatBytes(gptFit.minimum_bytes) }}</p>
              <p>Bu boyutta yeni bir RAW dosyası oluşturabilirsiniz.</p>
            </div>
            <div v-else>
              <strong>Küçük RAW imaj için GPT denetimi geçmedi</strong>
              <p>{{ localizeGptReason(gptFit.reason) }}</p>
            </div>
          </div>
          <div v-if="gptFit?.state === 'fits'" class="actions">
            <button type="button" class="button button-quiet" :disabled="!canUseFit" @click="openSizedImage">
              <FileArchive :size="17" aria-hidden="true" /> Bu boyutta RAW imaj al
            </button>
          </div>
        </form>
      </section>

      <template v-if="sourcePage">
        <section class="panel" aria-labelledby="source-title">
          <div class="panel-heading"><div class="panel-title"><HardDrive :size="19" aria-hidden="true" /><h2 id="source-title">Kaynak ayarları</h2></div></div>
          <form class="panel-body form" @submit.prevent="start">
            <fieldset class="source-choices">
              <legend>Kaynak türü</legend>
              <label>
                <input v-model="mode" type="radio" value="image">
                <FileArchive :size="18" aria-hidden="true" /> İmaj dosyası
              </label>
              <label>
                <input v-model="mode" type="radio" value="disk">
                <HardDrive :size="18" aria-hidden="true" /> Fiziksel disk
              </label>
            </fieldset>
            <template v-if="mode === 'image'">
              <label class="field">RAW dosyasının tam yolu
                <input v-model="path" type="text" placeholder="D:\Arşiv\Mac.raw" autocomplete="off" spellcheck="false">
              </label>
              <span class="quiet">Mantıksal sektör {{ sector }} B · <button class="text-button" type="button" @click="navigate('settings')">Ayarlar</button></span>
            </template>
            <template v-else>
              <label class="field">Fiziksel disk
                <select v-model="selected" :disabled="inventoryLoading">
                  <option :value="null">{{ inventoryLoading ? 'Diskler okunuyor…' : 'Disk seçin' }}</option>
                  <option v-for="disk in disks" :key="disk.number" :value="disk.number" :disabled="!disk.id">{{ diskLabel(disk) }}</option>
                </select>
              </label>
              <div v-if="sourceDisk && sourceDisk.offline !== true && page !== 'inspect'" class="notice notice-warning" role="status">
                <CircleAlert :size="17" aria-hidden="true" /><span>İmaj alma ve doğrulama için disk çevrimdışı olmalıdır.</span>
              </div>
              <span v-if="inventoryError" class="text-danger">{{ inventoryError }}</span>
            </template>
            <template v-if="imagePage">
              <div v-if="imagePreset" class="notice" :class="presetValid ? 'notice-neutral' : 'notice-warning'" role="status">
                <FileArchive :size="17" aria-hidden="true" />
                <div>
                  <strong>RAW imaj boyutu {{ formatBytes(imagePreset.target.bytes) }}</strong>
                  <p v-if="!presetValid">Disk seçimi veya geometrisi değişti. Klon planını yeniden denetleyin.</p>
                  <p v-else>{{ imagePreset.target.model }} kapasitesinde bir dosya oluşturulur.</p>
                  <p><button class="text-button" type="button" :disabled="busy" @click="imagePreset = null">Tam boyutlu imaja dön</button></p>
                </div>
              </div>
              <label class="field">İş klasörünün tam yolu
                <input v-model="directory" type="text" placeholder="E:\Kurtarma\Mac-001" autocomplete="off" spellcheck="false">
              </label>
              <label class="check"><input v-model="resume" type="checkbox"> Mevcut iş kaydından devam et</label>
              <p class="quiet">{{ resume ? 'Önceki işlemin klasörünü kullanın.' : 'Henüz var olmayan bir klasör adı girin.' }}</p>
              <p v-if="page === 'recovery'" class="quiet">Kurtarılamayan sektörler sıfırla doldurulur ve rapora işlenir.</p>
            </template>
            <div v-if="page === 'verify'" class="field">
              <label for="expected-digest">Beklenen SHA-256</label>
              <input id="expected-digest" v-model="expected" class="mono" type="text" placeholder="64 onaltılık karakter"
                autocomplete="off" spellcheck="false" :aria-invalid="invalidDigest" :aria-describedby="invalidDigest ? 'digest-error' : undefined">
              <span v-if="invalidDigest" id="digest-error" class="text-danger">64 karakter girin. Yalnızca 0–9 ve A–F kullanılabilir.</span>
            </div>
            <div class="actions">
              <button class="button button-primary" type="submit" :disabled="!canStart">
                {{ startLabel }} <ArrowRight :size="17" aria-hidden="true" />
              </button>
            </div>
          </form>
        </section>

        <section v-if="layout" class="panel" aria-labelledby="layout-title">
          <div class="panel-heading">
            <div class="panel-title"><ScanSearch :size="19" aria-hidden="true" /><h2 id="layout-title">{{ layout.table }}</h2></div>
            <span class="quiet">{{ layout.partitions.length }} bölüm</span>
          </div>
          <div v-if="layout.warnings.length || !layout.partitions.length" class="panel-body">
            <p v-if="!layout.partitions.length" class="quiet">{{ layout.filesystem }}</p>
            <div v-for="warning in layout.warnings" :key="warning" class="notice notice-warning">
              <CircleAlert :size="17" aria-hidden="true" /><span>{{ warning }}</span>
            </div>
          </div>
          <div v-if="layout.partitions.length" class="table-wrap" tabindex="0" role="region" aria-label="Bölüm tablosu">
            <table>
              <thead><tr><th scope="col">Bölüm</th><th scope="col">Başlangıç – son LBA</th><th scope="col">İmza</th><th scope="col"><span class="sr-only">İşlem</span></th></tr></thead>
              <tbody>
                <tr v-for="partition in layout.partitions" :key="partition.start_lba">
                  <th scope="row">{{ partition.label || partition.kind }}</th>
                  <td class="tabular">{{ partition.start_lba }} – {{ partition.end_lba }}</td>
                  <td>{{ partition.filesystem }}</td>
                  <td class="cell-action">
                    <button
                      v-if="partition.filesystem.startsWith('APFS')"
                      type="button"
                      class="button button-quiet button-small"
                      :disabled="busy"
                      @click="openApfsContainer(partition.start_lba, partition.end_lba)"
                    >
                      <Folder :size="15" aria-hidden="true" /> {{ action === 'apfs' ? 'Okunuyor…' : 'Konteyneri aç' }}
                    </button>
                  </td>
                </tr>
              </tbody>
            </table>
          </div>
        </section>

        <section v-if="apfsPartition" class="panel" aria-labelledby="apfs-title">
          <div class="panel-heading">
            <div class="panel-title"><Folder :size="19" aria-hidden="true" /><h2 id="apfs-title">APFS Konteyner İçeriği</h2></div>
            <span class="quiet">{{ apfsVolumes.length }} hacim</span>
          </div>
          <div v-if="apfsVolumes.length" class="table-wrap" tabindex="0" role="region" aria-label="APFS hacimleri">
            <table>
              <thead><tr><th scope="col">Hacim adı</th><th scope="col">Rol</th><th scope="col">Kullanılan alan</th><th scope="col">Durum</th><th scope="col"><span class="sr-only">İşlem</span></th></tr></thead>
              <tbody>
                <tr v-for="vol in apfsVolumes" :key="vol.index">
                  <th scope="row">{{ vol.name || `Hacim #${vol.index}` }}</th>
                  <td>{{ vol.role }}</td>
                  <td class="tabular">{{ formatBytes(vol.used_bytes) }}</td>
                  <td><span class="pill" :class="{ 'pill-neutral': vol.encrypted }">{{ vol.encrypted ? 'Şifreli' : 'Şifresiz' }}</span></td>
                  <td class="cell-action">
                    <button type="button" class="button button-quiet button-small" :disabled="busy || vol.encrypted" @click="browseApfs(vol, '/')">
                      <Folder :size="15" aria-hidden="true" /> Göz at
                    </button>
                  </td>
                </tr>
              </tbody>
            </table>
          </div>
          <div v-if="apfsVolume" class="panel-body form">
            <div class="actions">
              <strong class="mono">{{ apfsVolume.name }}:{{ apfsPath }}</strong>
              <button type="button" class="button button-quiet button-small" :disabled="busy || apfsPath === '/'" @click="apfsGoUp">Üst klasör</button>
            </div>
            <label class="field">Windows hedef klasörü (kopyalamak için)
              <input v-model="apfsDest" type="text" placeholder="D:\MacYedek" autocomplete="off" spellcheck="false">
            </label>
            <div class="actions">
              <button type="button" class="button button-primary" :disabled="busy || !apfsDest.trim()" @click="extractApfs(apfsPath)">
                <Download :size="16" aria-hidden="true" /> {{ action === 'apfs' ? 'Kopyalanıyor…' : 'Bu klasörü kopyala' }}
              </button>
            </div>
            <div v-if="apfsNotice" class="notice notice-success" role="status">
              <CircleCheck :size="17" aria-hidden="true" /><span>{{ apfsNotice }}</span>
            </div>
            <div v-if="apfsEntries.length" class="table-wrap" tabindex="0" role="region" aria-label="APFS dosya listesi">
              <table>
                <thead><tr><th scope="col">Ad</th><th scope="col">Tür</th><th scope="col">Boyut</th><th scope="col"><span class="sr-only">İşlem</span></th></tr></thead>
                <tbody>
                  <tr v-for="entry in apfsEntries" :key="entry.inode">
                    <th scope="row">
                      <button v-if="entry.kind === 'dir'" type="button" class="text-button" :disabled="busy" @click="browseApfs(apfsVolume, entry.path)">
                        {{ entry.name }}/
                      </button>
                      <span v-else>{{ entry.name }}</span>
                    </th>
                    <td>{{ entry.kind === 'dir' ? 'Klasör' : entry.kind === 'file' ? 'Dosya' : entry.kind }}</td>
                    <td class="tabular">{{ entry.kind === 'file' ? formatBytes(entry.size) : '—' }}</td>
                    <td class="cell-action">
                      <button
                        v-if="entry.kind === 'dir' || entry.kind === 'file'"
                        type="button"
                        class="button button-quiet button-small"
                        :disabled="busy || !apfsDest.trim()"
                        @click="extractApfs(entry.path)"
                      >
                        <Download :size="14" aria-hidden="true" /> Kopyala
                      </button>
                    </td>
                  </tr>
                </tbody>
              </table>
            </div>
            <p v-else class="quiet">Bu klasör boş.</p>
          </div>
        </section>
      </template>

      <section v-if="page === 'settings'" class="panel" aria-labelledby="settings-title">
        <div class="panel-heading"><div class="panel-title"><Settings2 :size="19" aria-hidden="true" /><h2 id="settings-title">Okuma ayarları</h2></div></div>
        <div class="panel-body form">
          <div class="field-grid">
            <label class="field">RAW dosyasının mantıksal sektör boyu
              <select v-model="sector"><option :value="512">512 B</option><option :value="4096">4096 B</option></select>
            </label>
            <label class="field">Transfer blok boyu
              <select v-model="block">
                <option v-for="n in [65536, 1048576, 4194304, 8388608]" :key="n" :value="n">{{ formatBytes(n) }}</option>
              </select>
            </label>
          </div>
          <p class="quiet">Bir işe devam ederken ilk işlemdeki sektör ve blok boyutlarını kullanın.</p>
          <p class="quiet">Uygulama sürümü: <strong class="mono">v{{ pkg.version }}</strong> · Starlit Studios</p>
        </div>
      </section>

      <section v-if="page === 'logs'" class="panel" aria-labelledby="logs-title">
        <div class="panel-heading"><div class="panel-title"><ClipboardList :size="19" aria-hidden="true" /><h2 id="logs-title">Oturum günlüğü</h2></div></div>
        <div class="panel-body">
          <p v-if="!activity.length" class="quiet">Bu oturumda henüz olay kaydedilmedi.</p>
          <ul v-else class="logs">
            <li v-for="(entry, index) in activity" :key="index">
              <span class="mono">{{ entry.timestamp }}</span><span>{{ entry.message }}</span>
            </li>
          </ul>
          <p class="quiet">Kalıcı iş kayıtları, imajın iş klasöründe saklanır.</p>
        </div>
      </section>

      <div v-if="statusError" class="notice notice-danger" role="alert">
        <CircleAlert :size="17" aria-hidden="true" /><span>İşlem durumu alınamadı. {{ statusError }}</span>
      </div>
      <div v-if="cancelError" class="notice notice-danger" role="alert">
        <CircleAlert :size="17" aria-hidden="true" /><span>Durdurma isteği gönderilemedi. {{ cancelError }}</span>
      </div>
      <OperationPanel v-if="status?.id" :status="status" :speed="speed" :stopping="stopping" @cancel="cancelOperation" />
    </main>
  </div>
</template>
