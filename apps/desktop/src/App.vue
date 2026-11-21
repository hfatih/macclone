<script setup lang="ts">
import {computed,onMounted,onUnmounted,ref,watch} from 'vue'
import {listen} from '@tauri-apps/api/event'
import logo from '../src-tauri/icons/icon.png'
import {api,type Disk,type Input,type Layout,type Status} from './api'
const tabs=['Genel bakış','Diskler','Klon planı','Disk imajı','Kurtarma','Dosya sistemi','Doğrulama','Günlük','Ayarlar'] as const
type Tab=typeof tabs[number]
const page=ref<Tab>('Genel bakış'),disks=ref<Disk[]>([]),inventoryError=ref(''),error=ref(''),loading=ref(false)
const mode=ref<'image'|'disk'>('image'),path=ref(''),selected=ref<number|null>(null),target=ref<number|null>(null),directory=ref(''),expected=ref(''),resume=ref(false)
const sector=ref(512),block=ref(1048576),layout=ref<Layout|null>(null),reasons=ref<string[]>([]),status=ref<Status|null>(null),activity=ref<string[]>([])
const disk=computed(()=>disks.value.find(d=>d.number===selected.value))
const subject=computed<Input|null>(()=>mode.value==='image'?(path.value.trim()?{kind:'image',path:path.value.trim(),sector:sector.value}:null):disk.value?.id?{kind:'disk',number:disk.value.number,id:disk.value.id}:null)
const busy=computed(()=>loading.value||status.value?.running)
const sourcePages:readonly Tab[]=['Disk imajı','Kurtarma','Dosya sistemi','Doğrulama']
const pct=computed(()=>{const p=status.value?.progress;return p?.total?Math.floor(100*p.bytes/p.total):0})
const speed=ref(0)
let selectionRevision=0
let inventoryRevision=0
let pendingStatus=false
let unlisten:(()=>void)|undefined, timer:ReturnType<typeof setInterval>|undefined, previous:{id:number;phase:string;bytes:number;time:number}|undefined
const fmt=(n:number)=>new Intl.NumberFormat('tr-TR',{maximumFractionDigits:1}).format(n/2**30)+' GiB'
const seen=(v:boolean|null)=>v==null?'Bilinmiyor':v?'Evet':'Hayır'
function log(text:string){activity.value=[new Date().toLocaleTimeString('tr-TR')+'  '+text,...activity.value].slice(0,80)}
function apply(s:Status){
 const current=status.value
 if(current&&(s.id<current.id||(s.id===current.id&&s.revision<=current.revision)))return
 const time=performance.now(),p=s.progress
 if(previous?.id===s.id && previous.phase===p.phase && p.bytes>previous.bytes && time>previous.time){speed.value=(p.bytes-previous.bytes)*1000/(time-previous.time)}
 else if(previous?.phase!==p.phase || previous.id!==s.id){speed.value=0}
 if(previous?.bytes!==p.bytes||previous?.phase!==p.phase||previous?.id!==s.id)previous={id:s.id,phase:p.phase,bytes:p.bytes,time}
 if(s.message && (status.value?.message!==s.message || status.value.id!==s.id))log(s.message)
 status.value=s
}
function isSameDisk(previous:Disk|undefined, next:Disk|undefined){
 return !!previous?.id&&!!next&&previous.id===next.id&&previous.serial===next.serial
}
async function refresh(){
 const run=++inventoryRevision
 inventoryError.value=''
 try{
   const found=await api.disks()
   if(run!==inventoryRevision)return
   if(selected.value!==null&&!isSameDisk(disk.value,found.find(d=>d.number===selected.value)))selected.value=null
   if(target.value!==null&&!isSameDisk(selectedTarget.value,found.find(d=>d.number===target.value)))target.value=null
   disks.value=found
 }catch(e){
   if(run!==inventoryRevision)return
   inventoryError.value=String(e)
   selected.value=null
   target.value=null
   disks.value=[]
 }
}
async function act(f:()=>Promise<void>){
 if(loading.value)return
 const version=selectionRevision
 error.value=''
 loading.value=true
 try{await f()}catch(e){
   if(version===selectionRevision)error.value=String(e)
   log(String(e))
 }finally{loading.value=false}
}
async function start(){
 const input=subject.value
 if(!input)return
 const version=selectionRevision
 await act(async()=>{
   if(page.value==='Dosya sistemi'){
     const result=await api.inspect(input)
     if(version===selectionRevision)layout.value=result
     return
   }
   if(page.value==='Doğrulama')await api.verify(input,expected.value.trim())
   else await api.image(input,directory.value,block.value,page.value==='Kurtarma',resume.value)
   apply(await api.status())
 })
}
const selectedTarget=computed(()=>disks.value.find(d=>d.number===target.value))
const layoutMode=computed(()=>{
 if(!disk.value||!selectedTarget.value)return undefined
 if(disk.value.bytes>selectedTarget.value.bytes)return 'smaller'
 if(disk.value.logical!==selectedTarget.value.logical)return 'sector'
 return disk.value.bytes===selectedTarget.value.bytes?'exact':'gpt'
})
function reverseDirection(){
 const source=selected.value
 selected.value=target.value
 target.value=source
}
const planChecked=ref(false)
async function plan(){
 const source=disks.value.find(d=>d.number===selected.value)
 const destination=selectedTarget.value
 const version=selectionRevision
 if(!source||!destination)return
 reasons.value=[]
 planChecked.value=false
 await act(async()=>{
   const blockers=await api.plan(source,destination)
   if(version===selectionRevision){
     reasons.value=blockers
     planChecked.value=true
   }
 })
}
function inspectDisk(d:Disk){if(busy.value)return;selected.value=d.number;mode.value='disk';page.value='Dosya sistemi';void start()}
watch([page,selected,target,path,sector,mode,disks],()=>{
 selectionRevision++
 error.value=''
 layout.value=null
 reasons.value=[]
 planChecked.value=false
},{flush:'sync'})
async function pollStatus(){
 if(pendingStatus)return
 pendingStatus=true
 try{apply(await api.status())}catch(e){error.value=String(e)}finally{pendingStatus=false}
}
onMounted(()=>{
 timer=setInterval(()=>{void pollStatus()},1500)
 void listen<Status>('operation',e=>apply(e.payload)).then(remove=>{unlisten=remove}).catch(e=>log(String(e)))
 void pollStatus()
 void refresh()
})
onUnmounted(()=>{unlisten?.();clearInterval(timer)})
</script>
<template>
<div class="shell">
 <aside><div class="brand"><img class="mark" :src="logo" alt=""> MacClone</div><nav><button v-for="t in tabs" :key="t" :class="{active:page===t}" @click="page=t">{{t}}</button></nav><small>Starlit Studios tarafından geliştirilmiştir</small></aside>
 <main><header><h1>{{page}}</h1><button v-if="page==='Genel bakış'||page==='Diskler'" @click="refresh">Diskleri yenile</button></header>
 <p v-if="error" role="alert" class="notice">{{error}}</p>
 <template v-if="page==='Genel bakış'||page==='Diskler'">
 <div v-if="page==='Genel bakış'" class="cards"><section><label>Fiziksel diskler</label><strong>{{inventoryError?'—':disks.length}}</strong></section><section><label>Geçerli işlem</label><strong>{{status?.running?'Çalışıyor':'Beklemede'}}</strong></section></div>
 <p v-if="inventoryError" class="notice">{{inventoryError}}</p>
 <section><h2>Disk envanteri</h2><p v-if="!disks.length&&!inventoryError">Depolama sağlayıcısı henüz disk bildirmedi.</p>
 <article v-for="d in disks" :key="d.number"><div><h3>{{d.model}} <small>Disk {{d.number}}</small></h3><p>{{fmt(d.bytes)}} · {{d.bus}} · {{d.health||'Sağlık bilinmiyor'}}</p>
 <p>Seri {{d.serial||'bilinmiyor'}} · Sektör {{d.logical}} / {{d.physical}} B · Salt okunur {{seen(d.readonly)}}</p><p>Sistem {{seen(d.system)}} · Önyükleme {{seen(d.boot)}} · Çevrimdışı {{seen(d.offline)}}</p></div><button :disabled="busy||!d.id" @click="inspectDisk(d)">İncele</button></article>
 </section><div class="actions"><button class="primary" @click="page='Disk imajı'">RAW imaj al</button><button @click="page='Klon planı'">Klon planı oluştur</button></div></template>
 <section v-if="page==='Klon planı'"><h2>Fiziksel disk uyumluluğu</h2><p>HDD → SSD ve SSD → HDD için kaynak ve hedefi seçin. Bu ekran salt okunur klon planı oluşturur.</p>
 <label>Kaynak disk<select v-model="selected"><option :value="null">Disk seçin</option><option v-for="d in disks" :key="d.number" :value="d.number">{{d.number}} · {{d.model}} · {{d.serial}} · {{fmt(d.bytes)}}</option></select></label>
 <label>Hedef disk<select v-model="target"><option :value="null">Disk seçin</option><option v-for="d in disks" :key="d.number" :value="d.number">{{d.number}} · {{d.model}} · {{d.serial}} · {{fmt(d.bytes)}}</option></select></label>
 <div class="actions">
 <button class="primary" :disabled="busy||selected===null||target===null" @click="plan">Planı denetle</button>
 <button :disabled="busy||selected===null||target===null||selected===target" @click="reverseDirection">Kaynak ve hedefi değiştir</button>
 </div>
 <p v-if="layoutMode==='gpt'">Daha büyük hedefte sağlam bir GPT ve açık GPT taşıma onayı gerekir. Bölüm sınırları korunur.</p>
 <p v-else-if="layoutMode==='exact'">Kapasiteler eşit. Plan tam disk kopyasını denetler.</p>
 <p v-else-if="layoutMode==='smaller'" class="notice">Hedef, kaynak diskin tüm sektörlerini alamayacak kadar küçük.</p>
 <p v-else-if="layoutMode==='sector'" class="notice">Mantıksal sektör boyları farklı.</p><p v-if="planChecked&&!reasons.length">Uyumluluk denetimi engel bulmadı.</p><ul v-if="reasons.length" aria-live="polite"><li v-for="reason in reasons" :key="reason">{{reason}}</li></ul></section>
 <template v-if="sourcePages.includes(page)"><section>
 <h2>Kaynak</h2><div class="seg"><button :class="{chosen:mode==='image'}" @click="mode='image'">İmaj dosyası</button><button :class="{chosen:mode==='disk'}" @click="mode='disk'">Fiziksel disk</button></div>
 <label v-if="mode==='image'">RAW dosyasının tam yolu<input v-model="path" placeholder="D:\Arşiv\Mac.raw" spellcheck="false"></label>
 <label v-else>Fiziksel disk<select v-model="selected"><option :value="null">Disk seçin</option><option v-for="d in disks" :key="d.number" :value="d.number">{{d.number}} · {{d.model}} · {{d.serial}} · {{fmt(d.bytes)}}</option></select></label>
 <p v-if="mode==='disk'">İmaj alma ve doğrulama için kaynak önceden çevrimdışı olmalıdır. Uygulama disk durumunu değiştirmez.</p>
 <template v-if="page==='Disk imajı'||page==='Kurtarma'"><label>İş klasörünün tam yolu<input v-model="directory" placeholder="E:\Kurtarma\Mac-001" spellcheck="false"></label>
 <label class="check"><input v-model="resume" type="checkbox"> Mevcut iş kaydından devam et</label>
 <p>{{page==='Kurtarma'?'Okunamayan sektörler yeniden denenir. Eksik kalan sektörlerin konumu kaydedilir ve imajda sıfırla doldurulur.':'Yeni iş için henüz var olmayan bir klasör seçin. İmaj ve doğrulama raporu burada oluşturulur.'}}</p></template>
 <label v-if="page==='Doğrulama'">Beklenen SHA-256<input v-model="expected" placeholder="64 onaltılık karakter" spellcheck="false"></label>
 <button class="primary" :disabled="busy||!subject||((page==='Kurtarma'||page==='Disk imajı')&&!directory.trim())||(page==='Doğrulama'&&!/^[a-f0-9]{64}$/i.test(expected.trim()))" @click="start">{{page==='Dosya sistemi'?'Bölüm tablosunu incele':page==='Doğrulama'?'İçeriği doğrula':resume?'İşe devam et':'İmaj al'}}</button>
 </section><section v-if="layout"><h2>{{layout.table}}</h2><p v-if="!layout.partitions.length">{{layout.filesystem}}</p><p v-for="w in layout.warnings" :key="w" class="notice">{{w}}</p><table v-if="layout.partitions.length"><thead><tr><th>Bölüm</th><th>Başlangıç – son LBA</th><th>İmza</th></tr></thead><tbody><tr v-for="p in layout.partitions" :key="p.start_lba"><td>{{p.label||p.kind}}</td><td>{{p.start_lba}} – {{p.end_lba}}</td><td>{{p.filesystem}}</td></tr></tbody></table></section></template>
 <section v-if="page==='Ayarlar'"><h2>Okuma ayarları</h2><label>RAW dosyasının mantıksal sektör boyu<select v-model="sector"><option :value="512">512 B</option><option :value="4096">4096 B</option></select></label><label>Transfer blok boyu<select v-model="block"><option v-for="n in [65536,1048576,4194304,8388608]" :key="n" :value="n">{{n/1024}} KiB</option></select></label><p>Devam ederken ilk işteki sektör ve blok boyunu seçin.</p></section>
 <section v-if="page==='Günlük'"><h2>Bu oturum</h2><p v-if="!activity.length">Henüz olay kaydedilmedi.</p><p v-for="(a,i) in activity" :key="i" class="mono">{{a}}</p><p>Kalıcı kayıtlar her iş klasöründe job.json, map.jsonl ve report.json dosyalarındadır.</p></section>
 <section v-if="status?.id" aria-live="polite"><h2>İşlem {{status.id}}</h2><p>{{({verify:'Hedefi yeniden okuma',copy:'Kopyalama',resume_check:'Önceki parçaları denetleme'} as Record<string,string>)[status.progress.phase]||'Hazırlık'}}</p><progress :value="pct" max="100"></progress><p>{{pct}}% · {{fmt(status.progress.bytes)}} / {{fmt(status.progress.total)}} · Eksik sektör {{status.progress.missing_sectors}}</p>
 <p v-if="status.running&&speed>0">{{(speed/1048576).toFixed(1)}} MiB/sn · Kalan yaklaşık {{Math.ceil((status.progress.total-status.progress.bytes)/speed)}} sn</p><button v-if="status.running" @click="act(()=>api.cancel(status!.id))">Güvenli noktada durdur</button><p>{{status.message}}</p>
 <template v-if="status.report"><h3>{{status.report.outcome==='partial'?'Eksik sektörlerle tamamlandı':'İmaj doğrulandı'}}</h3><p>Doğrulanan kaynak kapsamı {{fmt(status.report.verified_bytes)}}</p><code>{{status.report.sha256}}</code></template></section>
 </main></div>
</template>
