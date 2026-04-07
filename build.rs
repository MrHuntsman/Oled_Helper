// build.rs
// Embeds oled_helper.ico as ICON #1 and sets a PerMonitorV2-DPI-aware manifest.
fn main() {
    println!("cargo:rustc-link-lib=gdiplus");
    println!("cargo:rustc-link-lib=winmm");   // timeGetTime — high-resolution ms timer
    #[cfg(target_os = "windows")]
    {
        let mut res = winres::WindowsResource::new();
        res.set_icon_with_id("oled_helper.ico", "MAINICON");
        // Full PerMonitorV2 manifest:
        //   • <dpiAware>true/PM</dpiAware> covers Windows 8.1 and older loader paths
        //   • <dpiAwareness> with fallback chain covers Windows 10+
        //   • Both elements are required; omitting <dpiAware> causes bitmap-blur
        //     scaling on some Windows 10 builds even when <dpiAwareness> is present.
        //   • The fallback "PerMonitor" ensures Windows 8.1 compat.
        res.set_manifest(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">
        true/PM
      </dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">
        PerMonitorV2, PerMonitor
      </dpiAwareness>
    </windowsSettings>
  </application>
  <!-- comctl32 v6: required for visual styles (themed trackbar thumb, blue accent) -->
  <dependency>
    <dependentAssembly>
      <assemblyIdentity
        type="win32"
        name="Microsoft.Windows.Common-Controls"
        version="6.0.0.0"
        processorArchitecture="*"
        publicKeyToken="6595b64144ccf1df"
        language="*"
      />
    </dependentAssembly>
  </dependency>
</assembly>
"#);
        res.compile().expect("winres failed");
    }
}