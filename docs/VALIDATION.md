# Doğrulama kaydı · 21 Kasım 2026

| Kontrol | Gözlenen sonuç |
|---|---|
| Cargo format, Rust 1.94.1 | `cargo fmt --all -- --check` geçti. |
| Linux x64 çekirdek | 29 Rust testi geçti. Dağılım 6 aygıt/politika, 16 aktarım/kurtarma, 7 bölüm/FS. Kaynak ve hedef olarak dosya ve hata enjekte eden bellek aygıtı kullanıldı. |
| CLI uygulama yolu | `scripts/smoke.py` 512 ve 4096 B sektör için geçti. Her biri 8 MiB ve bir mantıksal sektör uzunluğunda iki sentetik disk kullanıldı. Kapsam: GPT/EFI/APFS işareti, dry-run, byte eşitliği, bağımsız Python SHA-256, continue, mevcut klasöre yazmayı reddetme, tek ve çift CRC arızası. |
| Referans motor | Sabit commit’ten derlenen `pm clone --sector`, 512 B düzeninde 8.389.120 bayt ve 4096 B düzeninde 8.392.704 bayt kopyaladı. İki çıktının tamamı, aynı kaynak ile MacClone imajına bayt bayt eşit çıktı. Referans aracın dosya okuyucusu üzerinde 4Kn biçimli bayt dizisi kullanıldı. Fiziksel 4Kn sürücü denenmedi. |
| Rust statik kontrol | Varsayılan workspace ve Windows x64 masaüstü için Clippy, uyarıları hata sayarak geçti. |
| Arayüz | Node 24, npm, ESLint, vue-tsc ve Vite üretim derlemesi geçti. Headless Chromium'da sentetik IPC ile gezinme, disk seçimi, GPT görüntüsü, dry-run planı, imaj isteği ve doğrulama girişleri sınandı. |
| Windows derleme | GNU x86_64 Windows hedefine çapraz derleme ve bağlama geçti. CLI ve masaüstü PE32+ x86-64 çıktısı oluşturuldu. Masaüstünde üretim web varlıkları `tauri/custom-protocol` ile gömüldü. |
| GitLab iş tanımı | .gitlab-ci.yml, GitLab’ın yayımladığı CI JSON şemasına göre doğrulandı. Temiz bir kaynak kopyasında Linux çekirdek/referans ve frontend işlerinin komutları yerel Linux ortamında çalıştırıldı. 29 test, iki bayt karşılaştırması, Clippy, lint ve üretim frontend derlemesi geçti. Uzak GitLab pipeline ve Windows paketleme işi bu aşamada çalıştırılmadı. |
| Yerel Windows kabulü | Windows 10/11 yürütmesi, WebView2 etkileşimi, Windows NSIS yükleyicisi, gerçek USB/SATA aygıtı ve Mac açılışı bu Linux ortamında sınanmadı. CI tanımlı; uzaktan workflow tetiklenmedi. |

Kurtarma fixture'ı bir sektörde deterministik medya hatası üretir. Normal mod durur, kurtarma modu tek sektörü eksik olarak haritalar. Hedef ve günlük yazma/flush hataları, checksum değişikliği, son satırda kesinti, yanlış kaynak, aynı işe eşzamanlı erişim, kaynak hard link'i ve mevcut hedef dizini testlerde kapsanır. Gerçek fiziksel diske yazılmadı.


## Referans karşılaştırmasını tekrarlama

Linux'ta depo kökünden şu komutlar referans sürümü derler ve iki CLI'yi aynı test girdileriyle çalıştırır. CI aynı commit'i checkout eder. Günlük uygulama derlemesi bu depoyu indirmez.

```sh
git clone https://github.com/inalto/partition-manager.git ../partition-manager
git -C ../partition-manager checkout --detach 1d6c43f69c198b5978c19e0d2b5722d28d138938
cargo build --locked --manifest-path ../partition-manager/Cargo.toml -p pm-cli
cargo build --locked -p macclone
python3 scripts/smoke.py target/debug/macclone --partition-manager ../partition-manager/target/debug/pm
```

Kurtarma günlüğüne girmeden hedefe ulaşan fazla baytların tekrar yazıldığı ayrıca sınandı. Son doğrulama bildiriminde iptal edilirse rapor yayımlanmıyor. Linux'ta hedefin doğrulama sırasında büyümesi ve bağımsız SHA-256 kontrolünde kaynak yolundaki dosyanın değiştirilmesi hata döndürdü.


## Windows volume extent değişikliği

Bu dalda Linux x64 üzerinde `cargo test --locked` toplam 39 testi geçti. 10 yeni test, harfsiz ve çok diskli volume, Windows ve EFI'nin ayrık diskleri, sayfa dosyaları ve döküm yapılandırması, optik/ağ filtreleme, değişen disk seçimi, kısa veya taşan yanıtlar ve erişim hatalarını kapsıyor. Her iki GPT fixture'ı da referans araçla tam bayt karşılaştırmasını tekrar geçti.

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo clippy --target x86_64-pc-windows-gnu -p macclone-desktop -p macclone -p mc-device --all-targets --locked -- -D warnings
cargo build --target x86_64-pc-windows-gnu -p macclone -p macclone-desktop --features tauri/custom-protocol --locked
```

Windows x64 çapraz derleme ve Clippy geçti. Masaüstü için `npm run lint` ve `npm run build` geçti. Chromium üzerinde sentetik Tauri yanıtıyla hedef diskin engel gerekçesi, seçilen disk kimliklerinin gönderilmesi ve seçim değişince eski sonucun temizlenmesi doğrulandı. Win32 ve PowerShell sorgularının etkin pagefile, EFI ve dinamik volume ile gerçek Windows üzerindeki çalışması yerel Windows kabulünde doğrulanacak.

## Fiziksel klon geliştirme adayı

- Linux üzerinde `cargo test --locked --features windows-physical-clone` ile **45 Rust testi** geçti. Yeni vakalar arasında süresi dolmuş veya yanlış aygıt onayı, sonradan beliren bağlı volume, günlük diski çakışması, yarım hedef yazımı, yanlış aygıtla devam denemesi ve geri okumada içerik uyuşmazlığı var.
- `scripts/smoke.py`, 512 ve 4096 bayt sektörlü GPT imajlarında önceki RAW yolunu ve sabit `partition-manager` sürümüne karşı byte eşitliğini tekrar doğruladı.
- Windows x64 GNU hedefi için aday CLI derlendi. Windows cihaz ve CLI kodu Clippy denetiminden geçti. VHDX kabul testi Windows x64 test ikilisine bağlandı. GitLab YAML şeması doğrulandı.
- VHDX testi Windows üzerinde **yürütülmedi**. İzole Windows testinde her sektör boyu için iki yeni, geçici VHDX oluşturulur. Ham kaynak tanıtıcısı okuma hakkıyla yeniden açılır. Hedefe aktarım ve flush sonrasında hedef başka bir tanıtıcıyla yeniden okunup tüm byte aralıkları karşılaştırılır. Test, işletim sisteminin kendi oluşturduğu VHDX için döndürdüğü aygıt yolunu kullanır. Normal test koşusunda bu kabul testi atlanır.
- Bu ortamda gerçek HDD/SSD yazımı, Windows GUI'den yetkili yardımcı, fiziksel USB/SATA bağlantı kaybı ve macOS açılışı denenmedi. Çevrimdışı ham diske erişim sürücüye ve Windows depolama politikasına bağlıdır. Aday CLI herhangi bir yazma yapmadan önce gerekli Win32 sorgusu ya da erişim başarısızlığını hata döndürür.

## MacClone ve Starlit Studios dalı

- 46 Rust testi geçti. Yerel plan testi, HDD ve SSD'yi iki yönde seçti. Daha büyük HDD'ye yönelince GPT taşıma modu seçildi. Aynı diskler ters çevrilip daha küçük SSD hedeflenince plan engellendi.
- CLI kabul testi her iki sektör boyunda eşit boyut için `partition-manager` ile bayt eşitliğini tekrar doğruladı. 512 ve 4096 bayt sektörlü GPT imajları önce bir, sonra 65 sektör büyütüldü. İki başlığın CRC'si Python ile bağımsız hesaplandı. Bölüm sınırları ve içerikleri aynı kaldı, koruyucu MBR ve yedek GPT yeni kapasiteye taşındı, yeni boş alan sıfırlandı.
- Aynı hedef boyutuyla devam ve farklı boyutla devamın reddi test edildi. Küçük hedef, hibrit MBR ve tek kopyası bozuk GPT daha büyük imaj oluşturamadı. Kaynak baytları değişmedi.
- Vue lint ve üretim derlemesi geçti. Chromium'da sentetik Tauri çağrılarıyla logo yükleme, Starlit Studios ibaresi, modellerin iki yönlü seçimi, kapasite uyarısı ve seçim değişince eski planın temizlenmesi doğrulandı.
- Windows x64 aday CLI ve MacClone masaüstü çapraz derlemeleri geçti. Fiziksel 512/4096 bayt sektörlü disk yazımı, yerel Windows VHDX kabulü ve macOS açılışı bu ortamda çalıştırılmadı.

## Doğrulama ve devam hataları

- 51 Rust testi geçti. Yeni regresyonlar, doğrulama sırasında aynı adla oluşan raporun korunmasını, eksik günlüğe sahip tamamlanmış işin yeniden yazmaya başlamamasını, bağlantı üzerinden rapor okunmasının engellenmesini, RAW kaynağın FIFO ile değiştirilmesini ve GPT ön sektörleri okunamadığında yedek tablodan incelemeyi kapsıyor.
- Gerçek CLI ile 512 ve 4096 bayt sektörlü GPT vakaları geçti. Eşit boyutta ham kopya `partition-manager` çıktısıyla bayt bayt eşleşti. Ek denetimler, MBR ve birincil GPT kaybından sonra incelemeyi ve ortak tablo dizisi kullanan geçersiz GPT yedeğinin bildirilmesini kapsıyor.
- Linux'ta 96 deterministik GPT/MBR mutasyonu ile CLI incelemesi kilitlenme veya Rust paniği üretmedi.
- Chromium'da sentetik Tauri çağrılarıyla aynı numaraya takılan yeni disk, geç gelen bölüm sonucu ve tamamlanmış işi eski bir ilerleme yanıtıyla geri alma senaryoları sınandı.
- Windows fiziksel klon adayı, işletim sistemi dosya önbelleğini atlayan sektör hizalı G/Ç kullanır. Aday CLI ve masaüstü Windows x64 için çapraz derlendi. Ham cihazlarda erişim modunun ve sürücü davranışının Windows çalışma zamanı doğrulaması bekliyor.

## Devam ve aygıt kimliği hata düzeltmeleri

- 56 Rust testi geçti. Ek regresyonlar geçerli başlangıcı olan yarım iş tanımını, işlem sırasında büyüyen RAW kaynağı, aygıt numarası değiştikten sonra kimlik ve kayıtlı blok doğrulamasını, tarama sırasında beliren yinelenen seri numarasını ve sanal/eksik/çakışan günlük depolamasını kapsıyor.
- Linux üzerinde 512 bayt bloklarla 4 MiB RAW kopyalayan ayrı CLI süreci ortada zorla sonlandırıldı. Tamamlanma raporu oluşmadığı doğrulandı. Aynı işte `--resume` çalıştırıldığında hedefin 4.194.304 baytı kaynakla eşleşti ve SHA-256 bağımsız olarak doğrulandı.
- 512 ve 4096 bayt sektörlü uçtan uca CLI senaryoları geçti. Eşit boyutlu imajlar `partition-manager` çıktısıyla bayt bayt aynıydı. Büyük GPT hedefi, yedek GPT CRC'si ve geçersiz hedefleri reddetme kontrolleri de geçti.
- Windows x64 CLI ve masaüstü çapraz derlendi. Ham okuma akışı çevrimdışı disk durumunu işlem sırasında ve son kaynak denetiminde sorgular. Gerçek Windows üzerinde USB çıkarıp takma, HDD/SSD aktarımı, çevrimdışı aygıt sürücüsü davranışı ve macOS açılışı çalışma zamanı doğrulaması bekliyor.

## Starlit masaüstü arayüzü

- ESLint, Vue tür denetimi ve Vite üretim derlemesi geçti. Linux üzerinde 56 Rust testi geçti. Windows x64 masaüstü için Clippy ve gömülü web varlıklarıyla çapraz derleme geçti.
- Chromium'da sentetik Tauri yanıtlarıyla SSD → HDD ve HDD → SSD planları, aynı disk engeli, envanter yenileme, aynı numaradaki aygıtın değişmesi ve geciken plan veya bölüm tablosu sonuçları sınandı.
- Klavyeden SHA-256 isteği, işlem başlayana kadar yinelenen gönderimin engellenmesi, eski durum bildirimleri, iptal hatasından sonra yeniden deneme, kısmi kurtarma ve imaj özetinin panoya kopyalanması sınandı.
- 1140 ve 820 piksel pencere genişliklerinde ana görünüm, 390 ve 375 piksel genişliklerde tablo ve plan formu denetlendi. Gerçek Windows WebView2 ve fiziksel aygıt kullanımı bu aşamada yürütülmedi.

## Küçük GPT RAW hedefi

- 56 Rust testi, Linux ve Windows x64 Clippy, Windows x64 CLI ve masaüstü çapraz derlemesi, ESLint ve Vite derlemesi geçti.
- 512 ve 4096 bayt sektörlü CLI senaryolarında imaj, GPT'nin izin verdiği en küçük hedefe yazıldı. Bölüm verileri, bölüm tablosu, iki başlığın CRC'si ve geri okuma SHA-256 değeri bağımsız doğrulandı. Bir sektör daha küçük hedef iş klasörü açmadan reddedildi. Aynı hedef boyutunda devam başarılı oldu, farklı boyut reddedildi. Eşit boyutlu imajlar sabit `partition-manager` çıktısıyla eşleşti.
- Mantıksal olarak 320 GB olan sentetik seyrek RAW dosyasında, son bölüm 250 GB konumunda bitiyorsa 256 GB ön denetimi geçti. Son bölüm 300 GB konumunda bitiyorsa ön denetim gereken minimum kapasiteyi bildirerek durdu. Denetim fiziksel aygıt kullanmadı.
- Masaüstünde sentetik Tauri yanıtlarıyla 320 GB → 256 GB hedefinde sığan ve sığmayan GPT yerleşimlerinin gösterimi doğrulandı. İmaj isteği SSD'den okunan tam bayt değerini taşıdı. Envanter yenilenip kapasite değiştiğinde eski planla imaj başlatılamadı. Windows x64 çapraz derlemesi geçti. Yerel WebView2 ve fiziksel SSD kullanımı çalıştırılmadı.
