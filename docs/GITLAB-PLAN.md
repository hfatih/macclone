# GitLab geliştirme planı

MacClone'un başlangıç noktası çalışan RAW imaj motorudur. Hedef, sağlıklı bir Mac HDD'nin bütün adreslenebilir sektörlerini SSD ve HDD arasında aynı mantıksal sektör boyuyla güvenle aktarmaktır. Büyük GPT hedefinde açık GPT taşıma modu bulunur. Dosya sistemi genişletme ayrı bir işlemdir. Fiziksel klon kullanıma açılana kadar çalışan `plan` komutu uyumluluk raporu üretir.

## 6.2 Sol'a ilk görev

GitLab issue şablonu `.gitlab/issue_templates/Fiziksel-klon-01.md` doğrudan uygulanabilir görev metnidir. GitLab'da yeni issue oluştururken **Fiziksel-klon-01** şablonu seçilebilir. Model seçimi kodlama oturumunun arayüzünde yapılır. GitLab CI test ve paketleme işlerini yürütür.

> Bu depoda .gitlab/issue_templates/Fiziksel-klon-01.md görevini uygula. Mimari ve kabul ölçütleri için docs/ARCHITECTURE.md ile docs/VALIDATION.md dosyalarını kullan. Sabit partition-manager sürümüne karşı mevcut RAW testlerini koru. Sonucu yeni bir dalda Draft MR olarak hazırla. Çalıştırdığın testleri ve Windows'ta henüz doğrulanmamış noktaları MR açıklamasına yaz.

## Uygulama sırası

| Sıra ve önerilen MR | Girdi | Teslim ve kapanış ölçütü |
|---|---|---|
| 1 · Windows volume ve disk eşlemesi | Mevcut salt okunur envanter | CLI ve GUI, tüm ilgili volume extent'leri üzerinden aynı planı üretir. İlk issue'nun olumsuz ve olumlu testleri geçer. |
| 2 · Planı aygıt kimliğine bağlama | MR 1'in topoloji bilgisi | Son kullanıcı kaynak/hedef modelini, serisini ve kapasiteyi görür. Onay plan kimliğine ve hedefe bağlanır. Takıp çıkarma, numara değişimi, kimlik çakışması, tekrar kullanılan onay ve journal'ın hedef diskte bulunması engellenir. Bu aşama fake aygıtlarla sınanır. |
| 3 · Ayrıcalıklı Windows yazma oturumu | MR 1 ve 2 | Dar yetkili yardımcı, kaynak ve hedefi açılan handle'lardan yeniden doğrular. GUI yükseltilmez. Onaylı hedef için kilit, yazma, flush ve geri okuma yaşam döngüsü izole Windows testinde oluşturulmuş VHDX çiftinde çalışır. Testin geçici VHDX'e ait olduğunu kanıtlayamadığı aygıta yazması engellenir. |
| 4 · Uçtan uca fiziksel klon | MR 3'te doğrulanmış oturum ve mc-engine | CLI ve masaüstünde plan → onay → kopya → doğrulama → rapor akışı çalışır. Kaynakta tek bir yazma çağrısı yapılmadığı, yarım parça, bağlantı kaybı, dolu günlük, iptal ve devamın doğru sonuç verdiği sınanır. Sağlıklı kaynak için sonuç tüm kaynak uzunluğunun geri okumasını ve SHA-256 eşitliğini içerir. |
| 5 · Windows kabulü ve sürüm | Önceki MR'ların birleşimi | Windows 10/11 x64, 512 B ve 4096 B senaryolarının sonuçları kaydedilir. VHDX, USB yeniden numaralama ve gerçek donanımda operatör tarafından yürütülen kabul ayrı belirtilir. Yalnızca doğrulanmış cihaz/geometri sınıfları kullanıcıya açılır. macOS açılış deneyi ayrı raporlanır. |

`inalto/partition-manager` v0.4.1 commit `1d6c43f69c198b5978c19e0d2b5722d28d138938` karşılaştırma sürümüdür. Tam disk doğrulamasında kullanılan komut `pm clone --sector`. Bölüm aralarında veri bulunan 512 B ve 4096 B GPT imajları iki ayrı motorla kopyalanır. Her iki çıktı da özgün kaynakla karşılaştırılır.

## GitLab CI ve runner'lar

- **core-and-reference** ve **frontend** dal ve MR pipeline'larında otomatik çalışır. Docker executor kullanan, etiketsiz işleri kabul eden bir Linux runner gerekir. Rust işi kilitli bağımlılıkları, sabit upstream commit'i, biçim, Clippy, birim testleri ve gerçek CLI karşılaştırmasını denetler. Node 24 işi npm kilit dosyasıyla lint ve üretim derlemesini çalıştırır.
- **windows-package** proje ya da grup CI değişkeni `MACCLONE_WINDOWS_RUNNER_TAG` bir Windows runner etiketine ayarlandığında dahil edilir. Runner gereksinimleri x64 Windows, PowerShell executor, Rust MSVC + Clippy, Visual Studio C++/SDK, Node 24/npm, Python 3 ve WebView2'dir. İş normal dosya testlerini çalıştırır ve NSIS yükleyicisini GitLab artifact'i olarak saklar. Etiketi olmayan bir pipeline Windows kabul kanıtı üretmez.
- VHDX kabulü MR 3 ile ayrı, izole bir Windows ortamına eklenir. Mevcut CI normal dosyalarla çalışır. Tüm testlerde fiziksel aygıta otomatik yazma yasaktır.
- `README.md` yerel derleme komutlarını, `docs/VALIDATION.md` gözlenen sonuçları içerir. Gerçek GitLab job bağlantıları ilk uzak pipeline tamamlandığında eklenebilir.

## Kaynak paketini projeye aktarma

`MacClone-gitlab.bundle` güncel `main` dalını, `feat/windows-volume-extents`, `feat/verified-physical-clone` ve `feat/starlit-macclone` geliştirme dallarını içerir. Aynı bundle GitHub projelerine aktarılabilir. Yeni ve boş GitLab projesi için yerelde `git clone -b main MacClone-gitlab.bundle macclone` ile başlayıp origin adresini kendi GitLab projenize ayarlayabilirsiniz. Var olan bir projeye aktarımda normal bir dal ve merge request kullanın. ZIP kullanılıyorsa arşiv içindeki `macclone` klasörünün içeriği, gizli `.gitlab` dizini ve `.gitlab-ci.yml` dahil GitLab deposunun köküne yerleştirilir.


## Fiziksel klon dalının durumu

`feat/verified-physical-clone` dalı volume denetiminin üstüne hedef onayı, günlük diskini kontrol eden Windows oturumu, varsayılan olarak kapalı HDD → SSD CLI ve ortak SHA-256 geri okumasını ekler. `MACCLONE_WINDOWS_VHDX_RUNNER_TAG` geçici VHDX kabulü için izole, yükseltilmiş Windows runner etiketine ayarlanırsa `windows-vhdx-acceptance` işi elle başlatılabilir. Test kendi oluşturduğu VHDX dışında aygıt yolu almaz. Gerçek Windows sonucu yayın kabul kaydına ayrıca yazılmalıdır.
