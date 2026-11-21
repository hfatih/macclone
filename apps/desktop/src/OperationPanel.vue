<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { Activity, Copy, Square } from '@lucide/vue'
import type { Status } from './api'
import { formatBytes, outcomeLabel } from './ui'

const props = defineProps<{ status: Status; speed: number; stopping: boolean }>()
const emit = defineEmits<{ cancel: [] }>()
const copyFeedback = ref('')
const digest = computed(() => props.status.report?.sha256)
const progress = computed(() => props.status.progress)
const percent = computed(() => {
  const { bytes, total } = progress.value
  return total > 0 ? Math.min(100, Math.max(0, Math.floor(bytes / total * 100))) : undefined
})
const operationTitle = computed(() => ({
  image: 'RAW imaj',
  recovery: 'Kurtarma',
  verify: 'SHA-256 doğrulaması',
})[props.status.operation ?? 'image'])
const phaseLabel = computed(() => {
  if (props.stopping) return 'Geçerli blok tamamlanıyor'
  const phase = progress.value.phase
  if (phase === 'copy') return 'Kopyalanıyor'
  if (phase === 'resume_check') return 'Önceki parçalar denetleniyor'
  if (phase === 'verify') return props.status.operation === 'verify' ? 'Dosya okunuyor' : 'Hedef yeniden okunuyor'
  return 'Hazırlanıyor'
})

watch(digest, () => { copyFeedback.value = '' })

async function copyDigest() {
  const hash = digest.value
  if (!hash) return
  try {
    await navigator.clipboard.writeText(hash)
    if (digest.value === hash) copyFeedback.value = 'SHA-256 kopyalandı.'
  } catch {
    if (digest.value === hash) copyFeedback.value = 'Panoya erişilemedi. Özeti seçerek kopyalayabilirsiniz.'
  }
}
</script>

<template>
  <section class="panel operation" aria-labelledby="operation-title">
    <div class="panel-heading">
      <div class="panel-title">
        <Activity :size="19" aria-hidden="true" />
        <h2 id="operation-title">{{ operationTitle }}</h2>
      </div>
      <span class="pill" :class="`outcome-${status.outcome}`" role="status">
        <span class="dot" aria-hidden="true"></span>
        {{ stopping ? 'Durduruluyor' : outcomeLabel[status.outcome] }}
      </span>
    </div>
    <div class="panel-body">
      <template v-if="status.running || progress.total > 0">
        <div class="progress-caption">
          <span>{{ status.running ? phaseLabel : 'İşlenen veri' }}</span>
          <strong class="tabular">{{ percent === undefined ? '—' : `%${percent}` }}</strong>
        </div>
        <progress :value="percent" max="100" :aria-label="`${operationTitle} ilerlemesi`"></progress>
        <div class="metrics tabular">
          <span>{{ formatBytes(progress.bytes) }} / {{ progress.total ? formatBytes(progress.total) : '—' }}</span>
          <span v-if="status.running && speed > 0">
            {{ formatBytes(speed) }}/sn · Yaklaşık {{ Math.ceil(Math.max(0, progress.total - progress.bytes) / speed) }} sn
          </span>
          <span v-if="progress.missing_sectors > 0" class="text-warning">Eksik sektör {{ progress.missing_sectors }}</span>
        </div>
      </template>
      <p v-if="status.message" class="result-note" :class="{ 'text-danger': status.outcome === 'failed' }" role="status">
        {{ status.message }}
      </p>
      <div v-if="digest && status.report" class="digest">
        <div class="progress-caption">
          <strong>SHA-256</strong>
          <button type="button" class="button button-quiet button-small" @click="copyDigest">
            <Copy :size="15" aria-hidden="true" /> Kopyala
          </button>
        </div>
        <code>{{ digest }}</code>
        <p class="quiet">Doğrulanan kapsam {{ formatBytes(status.report.verified_bytes) }}</p>
        <p v-if="copyFeedback" class="quiet" role="status">{{ copyFeedback }}</p>
      </div>
      <button v-if="status.running" type="button" class="button button-quiet stop-button" :disabled="stopping" @click="emit('cancel')">
        <Square :size="15" aria-hidden="true" /> Güvenli noktada durdur
      </button>
    </div>
  </section>
</template>
