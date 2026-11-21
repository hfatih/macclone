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
