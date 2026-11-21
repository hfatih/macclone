# Bağımlılık kararları

| Bileşen | Kullanım gerekçesi | Lisans |
|---|---|---|
| Rust std | RAII dosya tanıtıcıları, kilit, sınırlı tampon ve senkron disk I/O | Rust dağıtımı MIT / Apache-2.0 |
| Serde, serde_json | IPC, plan, makinece okunabilir rapor ve günlüğün sürümlenmesi | MIT OR Apache-2.0 |
| sha2 | İmaj ve okunan hedef için SHA-256, parça ve tam içerik doğrulaması | MIT OR Apache-2.0 |
| crc32fast | UEFI GPT başlığı ve bölüm dizisi IEEE CRC32 | MIT OR Apache-2.0 |
| windows-sys | Resmî Win32 tanımlarıyla sınırlı DeviceIoControl çağrıları | MIT OR Apache-2.0 |
| same-file | Açık dosya kimlikleriyle hard-link ve yol alias'larını karşılaştırma | MIT OR Unlicense |
| tracing / tracing-subscriber | CLI tanılama kaydı, veri içeriğini kaydetmeden | MIT |
| Tauri 2, tauri-build, JS API/CLI | Yerel masaüstü penceresi, dar IPC ve Windows paketleme | MIT OR Apache-2.0 |
| Vue 3 | Reaktif disk görünümü ve işlem durumu | MIT |
| TypeScript, vue-tsc | İstemci veri türleri ve şablon denetimi | Apache-2.0 / MIT |
| Vite, Vue Vite eklentisi, Tailwind CSS, PostCSS, Autoprefixer | Üretim varlıkları, stiller, çevrimdışı çalışan arayüz | MIT |
| ESLint, typescript-eslint, eslint-plugin-vue | Derlemeden bağımsız statik frontend denetimi | MIT |
| partition-manager CLI v0.4.1 (yalnızca karşılaştırma testi) | Sabit upstream commit ile tüm-disk RAW kopyasının bayt düzeyinde çapraz kontrolü. MacClone paketine bağlanmaz. | MIT |
| getrandom (isteğe bağlı) | Fiziksel klon onayında işletim sistemi rastgeleliği | MIT OR Apache-2.0 |
| tempfile (yalnızca test) | İzole ve temizlenen hata senaryoları | MIT OR Apache-2.0 |

Disk I/O tek sahipli, sıralı ve bloklayıcıdır. Tauri'nin async runtime'ı işleri arayüzden ayırır. Ayrı Tokio runtime, BLAKE3 veya bir filesystem sürücüsü eklemek ilk sürüm için gerekli değildir. Cargo.lock ve package-lock.json çözülmüş sürümleri sabitler. Doğrudan/transitif envanter için LICENSE_AUDIT.md dosyasına bakın.
