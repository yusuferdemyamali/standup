# StandUp — Linux MVP Teknik Spesifikasyonu

**Sürüm:** 0.1.0-MVP  
**Hedef platform:** Linux / Wayland / Hyprland (öncelik: CachyOS, iki monitör)  
**Gelecek olasılık:** Windows (MVP kapsamında değil)  
**Temel karar:** Tam ekran, çalışmayı kesen hatırlatma. Ön uyarı veya küçük bildirim yok.

## 1. Amaç

Kullanıcının bilgisayar başında aralıksız uzun süre oturmasını azaltmak için, **25 dakikalık çalışma süresinin sonunda tüm bağlı ekranlarda tam ekran hareket hatırlatması** göstermek. Kullanıcı **Harekete Başla** veya **5 Dakika Ertele** eylemlerinden birini seçer. Hareket molası 5 dakikadır. StandUp bir Pomodoro/iş takibi veya tıbbi takip ürünü değildir; yürüdüğünü fiziksel olarak ölçmez.

## 2. Ürün kararları

- Uygulama normalde arka planda sessizce çalışır; sürekli açık pencere yoktur.
- Varsayılan döngü: **25 dakika çalışma → tam ekran hatırlatma → 5 dakika mola → yeniden 25 dakika çalışma**.
- Hatırlatma göründüğünde çalışma akışı kesilir; bir ön uyarı verilmez.
- Hatırlatma ekranının iki ana eylemi: **Harekete Başla** ve **5 Dakika Ertele**.
- **Harekete Başla:** Katmanlar hemen kaldırılır, 5 dakikalık mola sayılır.
- **5 Dakika Ertele:** Katmanlar hemen kaldırılır, yalnızca 5 dakikalık erteleme sayılır. 25 dakikalık çalışma süresi sıfırlanmaz. Erteleme bitince hatırlatma tekrar açılır.
- Mola bittiğinde tam ekran yeniden açılmaz; kısa bir masaüstü bildirimi verilir ve yeni çalışma süresi başlar.
- Erteleme sayısına MVP'de sınır konmaz. Erteleme asla hareket molası olarak kaydedilmez.
- Çoklu monitörlü kullanımda hatırlatma **tüm bağlı monitörlerde** görünür; tüm yüzeyler aynı durumu gösterir. Birinde eylem seçilmesi hepsini kapatır.
- Bu katman **kilit ekranı değildir**: işletim sisteminin ekran kilidini veya güvenlik mekanizmalarını taklit etmez, kullanıcıyı sistem seviyesinde hapsedemez.

## 3. Ekran ve etkileşim

### 3.1 Hatırlatma arayüzü

- Arka plan: düz, koyu petrol/antrasit tonları. Vurgu: yumuşak yeşil.
- Orta bölgede sade ayak izi/hareket simgesi, büyük başlık **“Biraz hareket zamanı.”**, yardımcı metin **“25 dakikadır bilgisayar başındasın. Kalk, biraz yürü ve kendine 5 dakika ayır.”**.
- Birincil buton: **Harekete Başla**. İkincil buton: **5 Dakika Ertele**.
- Açılış geçişi yaklaşık 150–250 ms; animasyonlar “reduced motion” ayarına saygı duyar.
- HiDPI/ölçeklendirme (1080p laptop ve 1440p harici ekran) ve klavye ile buton erişimi çalışmalıdır.
- Arka plandaki uygulamaların etkileşimi, hatırlatma açıkken katmanların örttüğü bölgelerde kesilmelidir. Yalnızca bir katmanda keyboard-exclusive odak tutulmalı; diğer monitor yüzeyleri de mouse ile seçilebilir olmalıdır.
- Acil durum/erişilebilirlik: **Esc** tuşu **5 Dakika Ertele** ile aynı işi yapar; sistemin kilitlenmesi engellenir. Tam ekran uygulama her zaman kullanıcı tarafından sonlandırılabilir (ör. `standup --quit`); uygulama kendisini güvenlik amaçlı kilit mekanizması olarak sunmaz.

### 3.2 Mola davranışı

- Harekete Başla tıklanınca tam ekran kapanır. Kullanıcı hareket etmeye gidebilsin diye görünür 5 dakikalık pencere zorunlu değildir.
- Beş dakika sonra sistem bildirimi: **“Mola tamamlandı. Yeni çalışma döngüsü başladı.”**
- Bildirim servisi yoksa zamanlayıcı çalışmaya devam eder; bildirim eksikliği kritik hata değildir.

## 4. Durum makinesi

| Durum | Giriş olayı | Beklenen geçiş |
|---|---|---|
| `WORK` | Uygulama açılışı / molanın bitişi | 25 dk *aktif ve kilidi açık* oturum zamanı dolunca `REMINDER` |
| `REMINDER` | Çalışma süresi doldu / erteleme bitti | Tam ekran katmanları aç; Başla → `BREAK`, Ertele/Esc → `SNOOZE` |
| `SNOOZE` | Hatırlatma ertelendi | 5 dk dolunca tekrar `REMINDER`; çalışma sayacı sıfırlanmaz |
| `BREAK` | Harekete Başla | 5 dk dolunca bildirim ve `WORK` |
| `AWAY` | Ekran kilidi veya sistem askıya alınması | Süre hesabı, alttaki kurallarla yönetilir; kilit ekranının üstüne hatırlatma çizilmez |

`AWAY`, ana durumları silen bağımsız bir mola sayacı değil, ekran/oturum yaşam döngüsüne ait ek bağlamdır.

### 4.1 Uyku, ekran kilidi ve süre doğruluğu

- `WORK` süresinde ekran kilitli veya sistem askıdaysa çalışma sayacı ilerlemez.
- Kilitli/askıda geçirilen süre **5 dakika veya daha uzunsa**, kullanıcı geri döndüğünde yeni bir 25 dakikalık `WORK` döngüsü başlatılır. Bu, fiziksel egzersiz yapıldığını iddia etmez; yalnızca ekrandan uzaklaşmayı dikkate alır.
- Kilitli/askıda geçirilen süre **5 dakikadan kısaysa**, kalan çalışma süresi korunur.
- `REMINDER` açıkken ekran kilitlenirse overlay geçici olarak gizlenir. Geri dönüşte yukarıdaki 5 dakika eşiği uygulanır: kısa ayrılıkta aynı hatırlatma tekrar açılır, uzun ayrılıkta yeni `WORK` başlar.
- `SNOOZE` açıkken kısa kilit/askı süresi geçerse erteleme süresi korunur; kilit/askı sırasında erteleme sona erdiyse geri dönüşte hatırlatma açılır. En az 5 dakikalık ayrılıkta yeni `WORK` başlar.
- `BREAK` durumunda cihaz kilitli/askıda olabilir; molanın gerçek 5 dakikalık süresi sayılır. Süre dolmuşsa geri dönüşte `WORK` başlar; kilit üstüne hatırlatma çizilmez.
- Klavye/mouse hareketsizliğinden kullanıcının *gerçekten yürüdüğü* sonucu çıkarılmaz. Video izleme veya okuma yanlışlıkla hareket molası sayılmamalıdır.
- UI zamanlayıcısındaki gecikmeler birikimli sapmaya sebep olmamalı: süreler “her saniye 1 azalt” mantığına değil, monotonik saat ve kaydedilmiş son zaman sınırlarına dayanmalıdır. Uyku geçişleri için oturum/sistem sinyalleri ve güvenli duvar saati karşılaştırması kullanılmalıdır.
- Uygulama tamamen kapatılıp yeniden açıldığında v0.1'de yeni bir `WORK` döngüsü başlar; önceki çalışmanın kalan süresini kalıcı depolama MVP kapsamında değildir.

## 5. Teknik mimari

- **Dil:** Rust stable.
- **UI:** GTK4 / gtk-rs, CSS ile temalandırma.
- **Tam ekran katman:** `gtk4-layer-shell` (`Layer::Overlay`, dört kenara anchor, her monitöre ayrı yüzey, örnek tek aktif yüzeyde `KeyboardMode::Exclusive`). Katmanların tam ekran uygulamaların üstünde görünmesi gerçek Hyprland oturumunda test edilmeli; kullanıcının mevcut Hyprland ayarları zorla değiştirilmemeli.
- **Zamanlayıcı:** GTK/GLib ana döngüsüyle uyumlu; bağımsız saf Rust durum motoru. Zamanlama doğruluğu UI yenileme frekansına bağlı değildir.
- **Sistem yaşam döngüsü:** Linux D-Bus (`org.freedesktop.login1`/uygun oturum sinyalleri) ve Hyprland ile entegre ekran kilidi/suspend-resume takibi; desteklenmeyen sistem olaylarında tahmine dayalı sahte mola yazma.
- **Bildirim:** `org.freedesktop.Notifications` / masaüstü bildirim altyapısı.
- **Ayarlar:** `$XDG_CONFIG_HOME/standup/config.toml`, yoksa `~/.config/standup/config.toml`. GUI ayarlar ekranı zorunlu değil. Varsayılan süreler 25/5/5; sağlam doğrulama ve geçerli varsayılanlara dönüş olmalı.
- **Tek örnek:** İkinci kez başlatıldığında ikinci zamanlayıcı/overlay yaratma; var olan uygulama örneği korunmalı.
- **Otomatik başlangıç:** Kullanıcının açık tercihine bağlı. `standup.desktop` + mevcut oturumun XDG autostart yürütücüsü *veya* Hyprland `exec-once` yolu; iki yol birden etkinleştirilmemeli, sistem ayarları körlemesine değiştirilmemeli.
- **Dağıtım:** İlk hedef Arch/CachyOS x86_64; derleme/kurulum yönergesi, .desktop dosyası ve mümkünse basit paketleme betiği.
- **Ağ ve veri:** Ağ isteği, hesap, senkronizasyon, analitik, telemetri, bulut verisi yok.

Önerilen kaynak düzeni:

```text
standup/
  Cargo.toml
  src/
    main.rs
    domain/{mod.rs,timer.rs,state.rs,clock.rs}
    platform/{mod.rs,linux.rs,notifications.rs,session.rs}
    ui/{mod.rs,overlay.rs,styles.css}
    config.rs
  assets/standup.desktop
  docs/main_spec.md
  docs/phases/
  README.md
```

**Windows'a hazırlık:** Zamanlayıcı/durum makinesi `domain/` içinde platform-agnostik tutulur. Linux `layer-shell` ve bildirim/session kodları `platform/` katmanındadır; Windows'ta Win32/Windows App SDK ile yeni bir overlay adapter'ı yazılacaktır. GTK4 layer shell Windows'a taşınacakmış gibi varsayılmamalıdır.

## 6. İşlevsel kabul kriterleri

1. **AC-01:** Normal başlatmadan tam **25 dakika** sonra, kilidi açık aktif oturumda tüm bağlı monitörler üzerinde StandUp hatırlatması görünür; ön uyarı yoktur.
2. **AC-02:** Hatırlatma diğer olağan masaüstü uygulamalarının üzerinde açılır; kullanıcı iki belirgin butonu mouse veya klavye ile kullanabilir. Alt-F4/kilit/oturum değişimi gibi OS yetenekleri güvenlik kilidi gibi engellenmez.
3. **AC-03:** **5 Dakika Ertele** veya Esc overlay'i hemen kapatır; tam 5 dakika sonra yeniden gösterir, önceki 25 dakikayı başlatmaz.
4. **AC-04:** **Harekete Başla** overlay'i hemen kapatır, 5 dakikalık mola başlatır; mola bitince bildirim verir ve ardından yeni 25 dakikalık çalışma döngüsünü başlatır.
5. **AC-05:** 2+ monitörde görünüm eş zamanlıdır; bir ekrandaki eylem tüm overlay'leri kapatır. Monitör çıkarma/takma uygulamayı çökertmez.
6. **AC-06:** Kilit ve askıya alma durumunda overlay kilit ekranının üstünde durmaz. Kısa dönüşte kalan süre korunur; en az 5 dakikalık ayrılıkta yeni çalışma döngüsü başlar.
7. **AC-07:** Art arda erteleme, sistem saatinin değiştirilmesi veya yoğun CPU yükü hatalı çoklu hatırlatma ya da sayacın birikimli kaymasına neden olmaz.
8. **AC-08:** Aynı oturumda uygulamadan yalnızca bir örnek çalışır. Kapatma/çökme sonrası görüntü ve klavye odağı işletim sistemi tarafından geri kazanılır; kalıcı üst katman bırakılmaz.
9. **AC-09:** Kullanıcı onay verdiyse oturum açılışında tek kez başlar; otomatik başlangıç kapatılabilir. Kullanıcının mevcut Hyprland yapılandırması bozulmaz.
10. **AC-10:** 1080p ve 1440p, farklı ölçek faktörleri, klavye ile kullanım ve reduced motion kontrol edilir.
11. **AC-11:** Wayland layer shell bulunmadığında uygulama sessizce kısmi çalışan bir tam ekran göstermez; açıklayıcı hata ve düzgün çıkış sunar.
12. **AC-12:** İnternet olmadan çalışır; şifre, ağ, hesap veya uzaktaki servis gerektirmez.

## 7. MVP dışında

Windows sürümü; GNOME Wayland/X11 uyumluluk katmanı; istatistik paneli; alışkanlık puanı/seri sistemi; sağlık iddiası/egzersiz algılama; sistem tepsisi; GUI ile özelleştirme; Noctalia modülü; akıllı toplantı/oyun algılama; sesli uyarılar; bulut/hesap/telemetri.

## 8. İlk prototipte kapatılacak teknik riskler

1. Hyprland'da `Layer::Overlay` ve exclusive keyboard odağının tam ekran pencerelerin önünde çalışması.
2. İki monitör için tek durumlu, çok yüzeyli UI ve HiDPI davranışı.
3. Uyku / kilit yaşam döngüsü sinyallerinin mevcut CachyOS oturumunda gerçekten gözlemlenebilmesi.
4. Oturum başlangıcı yönteminin Noctalia/Hyprland yapılandırmasıyla çakışmaması.

**Referanslar**: 
- https://docs.rs/gtk4-layer-shell/latest/gtk4_layer_shell/trait.LayerShell.html
- https://docs.rs/gtk4-layer-shell/latest/gtk4_layer_shell/enum.KeyboardMode.html
- https://wiki.hypr.land/configuring/core/config-options/
- https://wiki.hypr.land/configuring/core/rules/layer-rules/
- https://wiki.archlinux.org/title/XDG_Autostart
- https://archlinux.org/packages/extra/x86_64/gtk4-layer-shell/files/
