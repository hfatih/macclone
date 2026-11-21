# Windows fiziksel klon planında volume extent denetimi

**Amaç.** Kullanıcı `macclone plan KAYNAK HEDEF` komutunu çalıştırdığında veya masaüstünde “Klon planı” açtığında, Windows'un koruduğu bir volume'un seçilen diske uzanıp uzanmadığı anlaşılmalı. Bir volume birden fazla diske yayılabilir. Disk harfi ve tek başına disk numarası bu karar için yetersizdir.

**6.2 Sol için çalışma kapsamı.** Bu issue'yu tek bir Draft MR'da uygula. Önce README.md, docs/ARCHITECTURE.md içindeki fiziksel hedef protokolü, mc-device içindeki mevcut plan ve Windows okuyucusu ile her iki istemcideki plan çağrısını oku. Mevcut 512 B / 4096 B karşılaştırma testlerini çalıştır. İnceleme referansı olarak aşağıdaki sabit upstream sürümü kullan. Yeni bir upstream bağımlılığı gerekiyorsa önce uygulama gerekçesini ve lisans etkisini MR'a yaz. [Windows oturum kodu](https://github.com/inalto/partition-manager/blob/1d6c43f69c198b5978c19e0d2b5722d28d138938/crates/pm-platform/src/windows/session.rs)

## İstenen değişiklik

- Windows adaptöründe yerel volume'ları GUID yollarından salt okunur sorgula. `IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS` yanıtındaki bütün extent'leri işle. Sürücü harfi bulunmayan volume'ları da kapsa. Kısa yanıtı, büyütülmesi gereken tamponu, erişim reddini ve aygıtın kaybolmasını açıkça ele al. Ağ sürücüsü ve optik medya gibi disk adayı olmayan girdilerin davranışını belgeleyip test et.
- İlgili Windows/boot/sistem, pagefile ve etkin crash-dump konumlarını disklerle eşleştir. Bir konumun yapılandırılmamış olması ile etkin bir konumun çözülememesi ayrı sonuçlar üretmeli. Sistemi etkileyen belirsizlikte plan engellenmeli. Güvenilir biçimde çözülemeyen bir Windows yapılandırması için destek sınırını raporla.
- Politika hesabını `mc-device` içinde test edilebilir tut. CLI ve Tauri aynı hesabı kullanmalı. Mevcut CLI JSON'unda `source`, `target`, `blockers` ve `physical_write_enabled` alanlarını koru. Yeni yapılandırılmış gerekçe kodları ve ilgili volume/disk kanıtları eklenebilir. Masaüstü seçilen hedefin engellenme nedenini göstermeli.
- Dry-run yalnızca gözlem yapmalı. Fiziksel yazma olanağı kapalı kalmalı. Bu MR mount, dismount, offline/online, format veya yazma yetkili disk handle'ı içermemeli. Mevcut `image`, `verify`, resume ve kurtarma işlerinin davranışını koru.

## Kabul testleri

- [ ] Windows dizini ve EFI sistem bölümü farklı disklerdeyken iki disk de korunur.
- [ ] Çok extent'li bir volume'un ikinci extent'i hedefe düştüğünde plan bunu bildirir. Sürücü harfi olmayan volume da yakalanır.
- [ ] Aktif pagefile veya dump depolama konumunda hedef seçimi engellenir. Çözülemeyen gerekli extent, geçersiz yanıt ve seçimden sonra kaybolan aygıt hata olarak görünür.
- [ ] Bilinen, ayrık, uyumlu sentetik disk çiftinde sahte engel üretilmez. Salt okunur planın sonucu fiziksel yazmayı etkinleştirmez.
- [ ] İlgili Win32 sorgusu Windows x64 için derlenir. Saf politika ve değişken uzunluktaki yanıtların ayrıştırılması sentetik veride sınanır.
- [ ] 29 testlik mevcut temel korunur. Eklenen testleri ve iki referans imajın tam bayt karşılaştırmasını MR'da raporla. Gerçek diske otomatik yazma testi çalıştırma.
- [ ] MR açıklamasında çalıştırılan komutları, alınan sonuçları ve gerçek Windows'ta henüz doğrulanmayan davranışları belirt.

Dal önerisi `feat/windows-volume-extents`. Sonraki iş ve bağımlılıklar `docs/GITLAB-PLAN.md` içinde.
