<script setup lang="ts">
import { HardDrive, ScanSearch } from '@lucide/vue'
import type { Disk } from './api'
import { formatBytes } from './ui'

defineProps<{ disks: Disk[]; busy: boolean }>()
const emit = defineEmits<{ inspect: [disk: Disk] }>()

const healthLabel: Record<string, string> = {
  Healthy: 'Sağlıklı',
  Warning: 'Uyarı',
  Unhealthy: 'Sorunlu',
  Unknown: 'Bilinmiyor',
}
</script>

<template>
  <div class="table-wrap" tabindex="0" role="region" aria-label="Disk envanteri">
    <table class="device-table">
      <thead>
        <tr>
          <th scope="col">Disk / Model</th>
          <th scope="col">Kapasite</th>
          <th scope="col">Bağlantı</th>
          <th scope="col">Durum</th>
          <th scope="col"><span class="sr-only">İşlemler</span></th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="disk in disks" :key="disk.number">
          <th scope="row">
            <div class="device-name">
              <span class="device-icon"><HardDrive :size="18" aria-hidden="true" /></span>
              <div>
                <strong>{{ disk.model || 'Model bilinmiyor' }}</strong>
                <span class="device-meta">Disk {{ disk.number }}<span v-if="disk.serial"> · {{ disk.serial }}</span></span>
              </div>
            </div>
          </th>
          <td>
            <span class="tabular">{{ formatBytes(disk.bytes) }}</span>
            <span class="device-meta">{{ disk.logical }} / {{ disk.physical }} B sektör</span>
          </td>
          <td>{{ disk.bus || 'Bilinmiyor' }}</td>
          <td>
            <span class="pill" :class="{ 'pill-neutral': disk.offline === true }">
              <span class="dot" aria-hidden="true"></span>
              {{ disk.offline == null ? 'Bilinmiyor' : disk.offline ? 'Çevrimdışı' : 'Çevrimiçi' }}
            </span>
            <span class="device-meta">
              <template v-if="disk.boot || disk.system">Sistem diski · </template>
              <template v-if="disk.readonly">Salt okunur · </template>
              {{ disk.health ? (healthLabel[disk.health] ?? disk.health) : 'Sağlık bilinmiyor' }}
            </span>
          </td>
          <td class="cell-action">
            <button
              type="button"
              class="button button-quiet button-small"
              :disabled="busy || !disk.id"
              :aria-label="`Disk ${disk.number} bölüm tablosunu incele`"
              :title="disk.id ? undefined : 'Disk kimliği alınamadı'"
              @click="emit('inspect', disk)"
            >
              <ScanSearch :size="16" aria-hidden="true" /> İncele
            </button>
          </td>
        </tr>
      </tbody>
    </table>
  </div>
</template>
