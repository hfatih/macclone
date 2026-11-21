# Sentetik test diskleri

`python scripts/make_fixture.py sample.raw` iki GPT kopyası, EFI bölümü ve ikinci bölümde NXSB imzası içeren deterministik disk üretir. `--sector 4096` 4Kn sürümüdür. Bölümlerde ve üç ayrılmamış aralıkta deterministik bayt desenleri bulunur. CLI testi 4 MiB sınırını iki kere geçen bir imaj ve sonda tek sektör kullanır. Kaynak ve hedef baytları scripts/smoke.py içinde karşılaştırılır. MBR, HFS+/HFSX, APM ve hata enjekte edilen aygıt örnekleri ilgili crate testlerinde bellekte üretilir. Hiçbir örnek gerçek kullanıcı verisi veya önyüklenebilir macOS içermez.
