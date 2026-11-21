# MacClone · mimari ve kabul sözleşmesi

## Tam disk kopyası için temel alınan uygulama

Proje kararı · 21 Kasım 2026. MacClone'un tam disk kopyası için davranış referansı MIT lisanslı `inalto/partition-manager` v0.4.1, commit `1d6c43f69c198b5978c19e0d2b5722d28d138938`. Tam kaynak seçilerek çağrılan `pm clone --sector` kullanımında `plan_used_map` değeri `None` olur. Sıfır hedef ofsetinde `copy_planned`, kaynak uzunluğunun tamamını `pm_image::copy_simple` işlevine geçirir. Bu yol hedefi flush eder ve ilk I/O hatasını çağırana iletir. `pm-ops` içinde bölümleri yeniden yerleştiren `CloneDisk` ise farklı bir işlemdir. [CLI kopyalama yolu](https://github.com/inalto/partition-manager/blob/1d6c43f69c198b5978c19e0d2b5722d28d138938/crates/pm-cli/src/cmd_clone.rs) · [Parça kopyalama döngüsü](https://github.com/inalto/partition-manager/blob/1d6c43f69c198b5978c19e0d2b5722d28d138938/crates/pm-image/src/rescue/engine.rs)

Bu seçimi yürütülebilir bir karşılaştırmaya bağladık. Linux CI sabit commit'teki CLI'yi kendi Cargo.lock dosyasıyla derler. Aynı sentetik RAW kaynakta referans uygulama ile MacClone çıktıları ayrı dosyalara yazılır. İki çıktı da özgün fixture ile bayt bayt karşılaştırılır. Kapsama EFI, bölüm araları, son bölümün arkasındaki alan, ikinci GPT ve son eksik transfer bloğu girer. Ayrı Rust testleri kayıp sektör, kesinti, yazma/flush hatası ve raporun yayımlanma koşullarını denetler. MacClone'un mevcut iş dosyaları ve devam etme biçimi korunur. Bir upstream sürüm güncellemesinde commit, lisans ve karşılaştırma sonucu birlikte gözden geçirilir.

Windows'ta fiziksel yazma tasarlanırken köprü ve sürücülerin bildirdiği extent'leri birleştiren `pm-platform/src/windows/session.rs` de başlangıç referansıdır. MacClone'da volume ayırma/onay ve kaynak diski salt okunur tutma kuralları kendi giriş noktasında uygulanır. [Windows oturum kodu](https://github.com/inalto/partition-manager/blob/1d6c43f69c198b5978c19e0d2b5722d28d138938/crates/pm-platform/src/windows/session.rs)

## İşlem sınırı

Bir Windows makinesine bağlı, bağımsız bir fiziksel mağaza üzerindeki Mac diskinin erişilebilir baytları işlenir. Kanıt birimi, mantıksal diskteki yarı açık [başlangıç, bitiş) bayt aralığıdır. EFI, bölümler arası boşluk ve yedek GPT bu uzayın içindedir. APFS parser'ının başarısızlığı ham veri okuma için engel oluşturmaz. Disk birleştiren Fusion, RAID, Storage Spaces veya donanım anahtarının bulunmadığı durumda tek mağazadan makinenin geri getirilebileceği sonucu çıkarılamaz.

## Sorumluluklar ve bağımlılık yönü

```
Vue → typed Tauri commands → application service ← CLI
                                  │
              inventory + policy ─┤
                                  ├─ bounded Read+Seek → partition / FS probes
                                  └─ transfer → flush → journal → reread → report
```

`mc-device` Windows envanterini ve kaynak tanıtıcılarını sahiplenir. `mc-inspect` yalnızca okuyucu alır. `mc-engine` plan, tamponlu transfer, kayıt ve doğrulama kurallarını içerir. CLI ve desktop aynı çekirdeği çağırır. İlk sürümde APFS/HFS+ birer küçük prob, kurtarma/doğrulama aynı transferin iki durumudur. Bunlara ayrı crate eklemek yerine yetkileri parametre türleri ve giriş noktaları sınırlar. Gelecek platformlar ayrı envanter ve tanıtıcı açma adaptörleri sunar.

## Karar tablosu

| Durum | Karar ve gerekçe |
|---|---|
| Aynı disk, alias, eksik/çakışan seri veya kimlik, bilinmeyen topoloji | Fiziksel yazma reddedilir. Disk numarası yol seçimidir ve kalıcı kimlik sayılmaz. |
| Boot/system/cluster, açık volume veya belirlenemeyen kritik bilgi | Fiziksel yazma reddedilir. Sistem, EFI ve Windows farklı disklerde olabilir. Disk 0 varsayımı kullanılamaz. |
| Kaynak canlı ve değişebilir | Tutarlı fiziksel kopya için kaynak önceden çevrimdışı olmalı. Uygulama volume bağlama, ayırma veya format işlemi yapmaz. |
| 512e → 4Kn veya 4Kn → 512 | Ham fiziksel aktarım reddedilir. LBA ve GPT anlamı değişir. 512n → 512e için mantıksal boy eşit olsa da fiziksel hizalama denetlenir. |
| Hedef küçük | Fiziksel klon engellenir. CLI RAW imajında açık hedef boyutu verilirse doğrulanmış GPT bölümlerinin tümü ve yedek tablo mevcut bölüm adresleriyle hedefe sığmalıdır. |
| Hedef büyük | Açık GPT taşıma onayı gerekir. İki GPT kopyası denetlenir. Koruyucu MBR ve başlıklar güncellenir, eski yedek metadata ve yeni boş alan sıfırlanır. Bölüm sınırları ve kimlikleri korunur. |
| Hasarlı GPT / bilinmeyen FS | İnceleme kısmi veya hatalı olarak görünür. Kaynak değişmeden imajlanabilir. |
| Okunamayan sektör | Normal aktarım durur. Kurtarma modunda sektör denemesi, açıkça kayıtlı sıfır dolgu ve eksik sonuç. Sıfır dolgunun hash'i kaynak sektörün hash'i değildir. |
| FileVault / SSV / APFS snapshot | Taşınabilen şifreli baytlar korunur. Anahtarla açılabilirlik, seal geçerliliği ve gerçek donanımda boot ayrı kanıt gerektirir. |

## Güven sınırları

Webview için keyfi shell, FS eklentisi, ham tanıtıcı ve yazılabilir cihaz yolu yoktur. Envanter salt okunur yapılandırılmış bir çağrıdır. İşletim sistemi katmanındaki sabit sorguya kullanıcı girdisi eklenmez. Disk seçimi, sayı ile kalıcı kimliği ve geometriyi eşler. Planlama hiçbir yazma tanıtıcısı açmaz. Gerçek hedef açılmadan önce seçim yeniden denetlenir. Kaynak hiçbir zaman yazma hakkı almaz.

Fiziksel yazma yolunun kabulü: iki ayrı güvenilir kimlik; tutulan tanıtıcılardan boy/geometri/kimlik; bütün volume extent'lerinde disjoint kontrolü; çalışan Windows için boot/system, pagefile ve dump konumları; açıkça gösterilen hedef seri/model/kapasite; tek kullanımlık plana bağlı onay; sektör hizalı I/O; hedef flush ve yeniden okuma. Çakışan topoloji ya da saptanamayan alias sınıfında sonuç `blocked`. Operatörün bir model adı yazması aygıt kimliği yerine geçmez. Kurulum dizininde imzalı, dar ACL'li broker ve oturuma bağlı IPC, GUI'den otomatik yükseltme için sonraki sınırdır. Yükseltilmiş webview varsayımı yapılmaz.

## Dayanıklılık protokolü

Kaynak kimliği, mantıksal sektör, kapasite, format sürümü, blok boyu ve kurtarma politikası iş kaydına bağlanır. Kaynaktan okunan parça için hedefe tüm yazma tamamlanır, hedef flush edilir, sonra kalıcı kayda bitiş konumu ve parça SHA-256'sı işlenir. İki adım arasında güç kesildiğinde son parça tekrar yazılabilir. Kayıttan önce diske ulaşmış baytlar tamamlanmış sayılmaz. Yırtık son kayıt için yalnızca eksik son çerçeve atılır. İç bozulma veya uyumsuz kaynak/hedef yeni iş gerektirir. Tek işin iki süreçten yürütülmesi OS dosya kilidiyle engellenir.

İptal, parça sınırında gözlenir. Hatalı yazma, flush veya günlük hatası başarı durumuna geçemez. Hedefteki kısmi veri kullanıcıya görünür. Tamamlama, kaynak kapsamı kadar hedefi okuyup SHA-256 karşılaştırır. Zarar görmüş kaynakta ek tam okuma diski yorabilir; ilk geçmişteki kaynak hash'i ile hedefin yeniden okunması ayrı kanıt olarak raporlanır. Resume sırasında doğrulanmış parçalar tekrar kontrol edilir. Büyük disklerde tampon sayısı sabit kalır ve kayıtlar akış halinde okunur. Depolama köprüsünün flush komutuna uymaması yazılımla kesin olarak giderilemez.

## Kullanıcı akışı ve kanıt

Kaynak → hedef → uyumluluk → bölüm görünümü → onay → transfer → geri okuma → indirilebilir iş raporu. Arayüz eksik telemetriyi “bilinmiyor” gösterir. İlerleme kalıcılaşan bayt üzerinden hesaplanır; tekrar deneme ve doğrulama ayrı fazlardır. Durum sorgusu pencere yenilendiğinde işlemi yeniden başlatmaz. Mac üzerinde doğrulanacaklar: hedefin tanınması, FileVault kilidini açma, System/Data eşleşmesi, Recovery/Preboot ve ilgili Mac modelindeki başlangıç güvenlik politikası. Bu koşullar rapordaki SHA-256 alanından bağımsızdır.

## Sürüm kapıları

1. Sentetik ve hata enjekte eden aygıtlarda sektör sınırları, kimlik, kesinti ve yanlış başarı bildirimi senaryoları.
2. Windows x64 CI, izole VHD/VHDX ile boot/sistem diski koruması, USB yeniden numaralama, GUI kapatma ve broker yaşam döngüsü.
3. Yalnız nitelendirilen boyut/geometri ve donanım sınıflarında fiziksel hedef açma. Otomatik testte gerçek fiziksel diske yazma yok.
4. APFS okuyucusu için sürümlenmiş macOS örnekleri, FileVault, snapshot, Fusion ve SSV olumsuz örnekleri. Yetenek sonucu `present / absent / unsupported / indeterminate` olarak ayrılır.

Uygulanmış sınırlar ve komutlar README dosyasında yer alır. Buradaki fiziksel yazma kabul şartları tamamlanmadan masaüstü uygulamasına yazılabilir disk bağlantısı verilmez.

## Uygulanan Windows topoloji denetimi

`mc-device::plan_disks` CLI ve masaüstünün kullandığı salt okunur planı üretir. Seçilen cihazların kimlik, geometri ve durum alanları tarama sonrası yeniden sorgulanır. CLI, mevcut JSON alanlarıyla birlikte `topology_issues` altında gerekçe kodunu ve mevcutsa disk numarası ve volume GUID'ini verir.

- Windows dizini işletim sisteminden alınır. Açılan dosya veya dizin tanıtıcısı yolu volume GUID'ine çözer. EFI önyükleme diski için `MSFT_Disk.IsSystem` kullanılır. Bu işaret harfsiz EFI bölümlerini de kapsar. EFI diskini belirleyemeyen kurulumda plan engellenir.
- `FindFirstVolumeW` ve takip eden tarama, sürücü harfi bulunmayan yerel volume'ları da kontrol eder. `IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS` yanıtı Windows x64 yerleşimine göre denetlenir. Tampon en fazla 4096 extent'e kadar büyütülür. Optik ve ağ olarak tanınan volume'lar atlanır. Tanınmayan, erişimi reddedilen, envanterle uyuşmayan veya aygıtı kaybolan bir volume gerekli topolojiyi belirsiz bırakır ve planı engeller.
- Etkin sayfa dosyaları `Win32_PageFileUsage` içinden gelir. Çökme dökümü için `CrashControl` modu, varsa `DedicatedDumpFile` ve moda bağlı çıktı yolu korunur. Devre dışı döküm modu bir konum gerektirmez. Etkin veya bilinmeyen bir modda çözülemeyen yapılandırma engeldir. Özel aygıt yolları ve işletim sisteminden güvenle genişletilemeyen yol değişkenleri bu sürümde çözümsüz raporlanır.
- Henüz oluşturulmamış döküm dosyası için mevcut üst dizinin volume'u sorgulanır. Açılan tanıtıcı üzerinden volume GUID'i çözüldüğünden bir NTFS mount point veya junction farklı bir diske uzanıyorsa o disk değerlendirilir. Yerleşimin kararlı olduğu gerçek yazma oturumu sonraki aşamanın konusu olarak kalır.

## Teknik dayanaklar

[UEFI GPT yerleşimi](https://uefi.org/specs/UEFI/2.10/05_GUID_Partition_Table_Format.html) · [Microsoft MSFT_Disk](https://learn.microsoft.com/en-us/windows-hardware/drivers/storage/msft-disk) · [CreateFileW](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew) · [volume disk extents](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-ioctl_volume_get_volume_disk_extents) · [Apple Signed System Volume](https://support.apple.com/guide/security/signed-system-volume-security-secd698747c9/web)

## Fiziksel hedef için uygulanacak protokol

İsteğe bağlı Windows CLI, fiziksel aktarım protokolünün ilk uygulamasıdır. Operatör modeli, seriyi, kimliği ve kapasiteyi görür. Rastgele onay kodu beş dakika içinde tek kullanımlık plana bağlanır. Plan yeniden doğrulanır ve günlük dosyalarının bulunduğu volume'un bütün disk aralıkları kaynak/hedef ile karşılaştırılır. Ham tanıtıcılar açıkken disk numarası, uzunluk, bağlantı türü, seri, hizalama ve çevrimdışı durum sorgulanır. Kopyalama aynı günlüğü, flush sırasını ve geri okumayı kullanır. Aday CLI yönetici PowerShell oturumunda çalışır. Tauri salt okunur plan sunar. GUI için ayrıcalıklı yardımcı ve yönetici oturumuyla Windows kabulü ayrı aşamalardır.

1. **Envanter.** Disk numarası, PnP/aygıt kimliği, seri, bus, model ve geometri ayrı alanlardır. `MSFT_Disk.IsBoot` ve `IsSystem` iki ayrı girdidir. Windows dizini, sistem/EFI, sayfa dosyaları, dump ve günlük/imajı barındıran volume'lar fiziksel extent kümelerine çözülür. Her korunmuş volume'un tüm extent'leri kapsanır. GUID, sürücü harfi veya tek extent varsayımı yapılamaz.
2. **Salt okunur plan.** Plan kimlikleri, topoloji nesli, imaj/map host diski, kaynak/hedef sektör boyları, kapsam, ayar ve plan nonce'unu bağlar. Planın önerdiği hedef ve kapasite bayt cinsinden gösterilir. Model/seri eksikliği, yinelenen USB seri numarası veya eşlenemeyen bir volume varsa yürütme hakkı oluşmaz. Partition-only aktarım, tüm diski doğrulama politikasıyla aynı komut olmayacaktır.
3. **Onay ve yürütme.** Kullanıcı hedef modeli, seri sonu ve kapasiteyi gördükten sonra plana özel hedef ifadesini girer. Yükseltilmiş yardımcı, istek sahibinin SID'sine sınırlandırılmış yerel IPC oturumu kullanır. Plan tek işlemde tüketilir. Disk çıkarma, yeniden takma veya hedef seçim değişimi onayı düşürür. Yardımcı sayısal disk referansından yolu kendisi üretir.
4. **Açık tanıtıcılara bağlama.** Kaynak `GENERIC_READ` ile açılır. Hedefin seri, aygıt numarası, depolama kimliği, uzunluk ve hizalaması eldeki tanıtıcılardan tekrar okunur. Volume ve disk oturumları işlem sonuna kadar tutulur. Lock veya dismount gerekirse onay ayrı gösterilir. Kayıp topoloji, kilit ya da aygıt hatası sonrasında yeni aygıt numarasına otomatik geçilmez.
5. **Bitirme.** Hedef flush, kanıtlanan aralıkların yeniden okunması ve terminal raporu sırayla tamamlanır. Hedef çevrimiçine otomatik alınmaz. Ham kopya kaynak GPT disk/partition GUID'lerini tekrarlar. Eşzamanlı takılı disklerin Windows tarafından aynı imzalı görülmesi raporda belirtilir. Kimlik yeniden oluşturma, yedek GPT taşıma veya bölüm büyütme ayrı bir kullanıcı işlemi gerektirir.

| Windows arayüzü | Sorumluluk ve sınır |
|---|---|
| CreateFileW / Rust OpenOptionsExt | Bugün yalnız okuma hakkı ile PhysicalDrive açma. Erişim reddi tanı olarak dışarı çıkar. |
| DeviceIoControl + IOCTL_DISK_GET_LENGTH_INFO | Açık handle üzerindeki byte uzunluğunu doğrula. Uzunluk bir filesystem kullanım sayacı değildir. |
| IOCTL_STORAGE_QUERY_PROPERTY | Device descriptor seri bilgisi ve AccessAlignment logical/physical alanlarını karşılaştır. Desteklenmeyen sürücüde tutarlı fiziksel imajlama reddedilir. |
| IOCTL_STORAGE_GET_DEVICE_NUMBER | Açık handle'ın numarasını seçimle denetle. Tek başına kimlik kanıtı sayılmaz. |
| IOCTL_DISK_GET_DRIVE_GEOMETRY_EX | İkincil geometri kontrol adayı. Güncel kaynakta uzunluk ve alignment descriptor kullanılır. |
| IOCTL_DISK_GET_PARTITION_INFO_EX | Belirli bölüm handle'ının OS görünümü. Birincil/ikincil GPT CRC denetimi için ham okuyucunun yerini tutmaz. |
| IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS | Gelecek broker için değişken sayıda extent, büyütülmüş yanıt ve üst sınır. Spanned/dinamik volume'ları belirsiz say. |
| FlushFileBuffers / Rust File::sync_all | Bugün normal dosya ve günlük kalıcılığı. Fiziksel hedefte ayrıca kilitli, salt okunabilir hedef doğrulama oturumu gerekecek. |

## Kurtarma, veri kökeni ve APFS okuyucu sınırı

İmajdaki sıfır, (1) kaynaktan başarıyla okunan sıfır, (2) kesin medya hatası sonrası dolgu veya (3) henüz kopyalanmamış alan olabilir. Rapor, map ve iş durumu birlikte bu üç durumu ayırır. İş açıldığında tamamlanmış, kalıcı bir checkpoint yoksa yüzde yüz sıfır görünen bir dosyanın okunmuş olduğu varsayılmaz. Güvenilmeyen bir iş klasörünü başka kullanıcılar arasında güvenli biçimde paylaşmak bu yerel tasarımın kapsamı dışındadır. Checksum rastlantısal bozulmayı saptar; dijital imza değildir.

İleride APFS adaptörü yalnızca bölümle sınırlı bir okuyucu alacak. NXSB işareti önce bir aday üretir. Desteklenen blok boyu ve container sınırları, nesne checksum'ları, uyumlu feature bitleri, tutarlı checkpoint/XID ve OMAP çözülmeden volume sağlığına geçilmez. Volume rolleri isimden tahmin edilmez. Snapshot sorgusu yürümüyor veya anahtar yoksa sonuç belirsiz/şifreli döner. Recovery/Preboot varlığı bir başlangıç politikası testi sayılamaz. HFS+ ve HFSX tarafında volume header, taşma extent'leri ve katalog ayrı testlerle açılır. Journal replay kaynakta çalıştırılmaz.

## Performans bütçesi ve kabul ölçüleri

İlk motorun blok boyu 512 B veya 4096 B mantıksal sektöre hizalı, en çok 8 MiB'dir. Her blokta iki kalıcı yazma bariyeri vardır. Bu tercih küçük blokta özellikle USB HDD performansını düşürür. Gruplu checkpoint optimizasyonunun kabul koşulu herhangi bir güç kesiminde map'in kalıcı verinin önüne geçmemesidir. Ek bellek gereksinimi transfer bloğu, tek journal satırı ve fiziksel okuyucunun 64 KiB hizalı tamponuyla sınırlıdır. GPT dizi limiti ayrı olarak 16 MiB ve 16.384 girdidir.

Başarı ölçüleri: aradaki boşluklara yazılan sentineller dahil kaynak aralığının tam eşitliği; başlangıçta ve resume sonunda kaynak hash'inin değişmemesi; iptalin kalıcı son parçayı aşmaması; hedef veya günlük flush hatasında başarı raporunun üretilmemesi; tek sektörü kayıp bir işte doğrulanmış kaynak kapsamının tam bir mantıksal sektör eksilmesi. Yerel testten geçen bu motorun fiziksel restore yetkisi, Windows VHDX ve hot-plug kabul deneylerinden bağımsız olarak açılamaz.


## GPT hedef boyutu

`mc-inspect::GptRelocation` salt okunur beklenen hedef akışını oluşturur. Koruyucu MBR ve iki geçerli GPT kopyası, aynı bölüm dizisi, geçerli bölüm aralıkları ve GPT yedeğinin kaynak sonundaki konumu şarttır. Hibrit MBR, kayıp kopya, çakışan bölümler veya hizasız hedef reddedilir. Akış aynı veri bloklarını taşır ve yalnızca GPT/MBR metadata dönüşümlerini ve sıfır aralıklarını uygular. Eski yedek GPT ile yeni yedek alanın çakıştığı tek sektör büyüme de desteklenir.

Küçük hedef için gerekli minimum bayt sayısı, son GPT bölümünün bitişi ve korunacak yedek GPT yerleşiminden hesaplanır. Hedef bu sınırın altındaysa iş klasörü açılmadan hata verilir. Sığıyorsa RAW imaj akışı ayrılmamış son alanı dışarıda bırakır. Dosya sistemi yeniden boyutlandırma ve küçük fiziksel hedefe yazma bu akışın kapsamı dışındadır.

Günlük kaynak ve hedef boyutlarını birlikte saklar. Devam sırasında GPT beklenen akışı aynı kaynaktan yeniden hazırlanır. Rapor, GPT taşınmışsa kaynak boyutunu `gpt_relocated_from_bytes` alanıyla bildirir. Bu modda `verified_bytes` dönüştürülmüş hedef içeriğinin doğrulanan uzunluğudur. Bölüm içi dosya sistemi kapsamı ve bölüm konumu aynı kalır. Tam eşit boyutta eski RAW sözleşmesi geçerlidir.
