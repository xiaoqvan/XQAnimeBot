//! Ultralight 2.0 C API 绑定（运行时动态加载）。
//!
//! 为什么不用 `#[link]` / build script 链接：和 FuyuBot 加载 tdjson 的哲学一致——
//! 运行时找不到就报一条能看懂的错，而不是构建期就炸；同一个二进制也能配不同版本的 SDK。
//!
//! 所有签名都对着 SDK 2.0.0-beta.2 的 `include/Ultralight/CAPI/*.h` 抄的，
//! 改 SDK 版本时请以头文件为准，别凭记忆改。

use std::ffi::{CStr, CString, c_char, c_void};
use std::path::{Path, PathBuf};

use libloading::Library;

/// 不透明句柄（ULRenderer / ULView / ULString / ULConfig…）。
pub type Handle = *mut c_void;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("找不到 Ultralight 运行时：{0}")]
    RuntimeMissing(String),
    #[error("Ultralight 运行时加载失败：{0}")]
    Load(String),
    #[error("Ultralight 渲染失败：{0}")]
    Render(String),
    #[error("页面在 {0} ms 内没有就绪（可能是 JS 报错或模板太大）")]
    Timeout(u64),
    #[error("输出图片编码失败：{0}")]
    Image(#[from] image::ImageError),
    #[error("文件读写失败（{path}）：{source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
}

pub type Result<T> = std::result::Result<T, Error>;

/// 解析一个符号，名字打错时给出带函数名的错误（而不是 `GetProcAddress failed`）。
macro_rules! sym {
    ($lib:expr, $name:literal, $ty:ty) => {{
        #[allow(unused_unsafe)]
        let symbol: libloading::Symbol<'_, $ty> =
            unsafe { $lib.get(concat!($name, "\0").as_bytes()) }
                .map_err(|err| Error::Load(format!("{}: {err}", $name)))?;
        *symbol
    }};
}

/// 已加载的 Ultralight。持有库句柄与解析好的函数指针。
///
/// 生命周期：**必须在主线程创建和使用**（Ultralight 的硬性要求），一个进程一个实例。
pub struct Ultra {
    // 库必须活得比函数指针长，所以泄漏成 'static（进程结束时随系统回收）。
    _core: &'static Library,
    _appcore: Option<&'static Library>,

    pub dir: PathBuf,

    // ── 字符串 ──
    create_string: unsafe extern "C" fn(*const c_char, usize) -> Handle,
    destroy_string: unsafe extern "C" fn(Handle),
    string_data: unsafe extern "C" fn(Handle) -> *mut c_char,
    string_is_empty: unsafe extern "C" fn(Handle) -> bool,

    // ── 配置 ──
    create_config: unsafe extern "C" fn() -> Handle,
    destroy_config: unsafe extern "C" fn(Handle),
    config_set_resource_prefix: unsafe extern "C" fn(Handle, Handle),
    config_set_cache_path: unsafe extern "C" fn(Handle, Handle),
    config_set_settle_delay: Option<unsafe extern "C" fn(Handle, f64)>,
    config_set_min_log_level: Option<unsafe extern "C" fn(Handle, u32)>,

    // ── Renderer ──
    create_renderer: unsafe extern "C" fn(Handle) -> Handle,
    destroy_renderer: unsafe extern "C" fn(Handle),
    update: unsafe extern "C" fn(Handle),
    refresh_display: unsafe extern "C" fn(Handle, u32),
    render: unsafe extern "C" fn(Handle),

    // ── View ──
    create_view_config: unsafe extern "C" fn() -> Handle,
    destroy_view_config: unsafe extern "C" fn(Handle),
    view_config_set_accelerated: unsafe extern "C" fn(Handle, bool),
    view_config_set_javascript: unsafe extern "C" fn(Handle, bool),
    default_session: unsafe extern "C" fn(Handle) -> Handle,
    create_view: unsafe extern "C" fn(Handle, u32, u32, Handle, Handle) -> Handle,
    destroy_view: unsafe extern "C" fn(Handle),
    view_load_html: unsafe extern "C" fn(Handle, Handle, Handle, bool),
    view_set_device_scale: unsafe extern "C" fn(Handle, f64),
    view_is_settled: unsafe extern "C" fn(Handle) -> bool,
    view_evaluate_script: unsafe extern "C" fn(Handle, Handle, *mut Handle) -> Handle,
    view_get_surface: unsafe extern "C" fn(Handle) -> Handle,

    // ── Surface ──
    surface_width: unsafe extern "C" fn(Handle) -> u32,
    surface_height: unsafe extern "C" fn(Handle) -> u32,
    surface_row_bytes: unsafe extern "C" fn(Handle) -> u32,
    surface_lock: unsafe extern "C" fn(Handle) -> *mut c_void,
    surface_unlock: unsafe extern "C" fn(Handle),
}

impl Ultra {
    /// 加载运行时。`dir` 里应该有 4 个 DLL 和 `resources/`。
    pub fn load(dir: &Path) -> Result<Self> {
        let lib_path = |name: &str| -> PathBuf {
            if cfg!(windows) {
                dir.join(format!("{name}.dll"))
            } else if cfg!(target_os = "macos") {
                dir.join(format!("lib{name}.dylib"))
            } else {
                dir.join(format!("lib{name}.so"))
            }
        };

        let core_path = lib_path("Ultralight");
        let appcore_path = lib_path("AppCore");
        if !core_path.exists() {
            return Err(Error::RuntimeMissing(format!(
                "{} 不存在。把 Ultralight 2.0 SDK 的 bin/ 与 resources/ 放到 {} 下\
                 （scripts/fetch-ultralight.ps1 可以自动拉取）",
                core_path.display(),
                dir.display()
            )));
        }

        // AppCore.dll 依赖同目录的 Ultralight.dll / WebCore.dll / UltralightCore.dll。
        // Windows 上必须带「用库自身所在目录搜依赖」的 flag，否则同目录的兄弟 DLL 找不到。
        let core = unsafe { load_library(&core_path)? };
        let appcore = if appcore_path.exists() {
            Some(unsafe { load_library(&appcore_path)? })
        } else {
            None
        };

        // 默认平台实现（文件系统 / 字体 / 剪贴板）只在 AppCore 里，必须先注册再建 Renderer。
        if let Some(appcore) = appcore {
            unsafe {
                if let Ok(enable_fonts) =
                    appcore.get::<unsafe extern "C" fn()>(b"ulEnablePlatformFontLoader\0")
                {
                    enable_fonts();
                }
                if let Ok(enable_fs) =
                    appcore.get::<unsafe extern "C" fn(Handle)>(b"ulEnablePlatformFileSystem\0")
                {
                    // base_dir 留空：我们只用 LoadHTML，不读 file:// 资源。
                    let empty = CString::new("").expect("空字符串");
                    let s = sym!(
                        core,
                        "ulCreateStringUTF8",
                        unsafe extern "C" fn(*const c_char, usize) -> Handle
                    );
                    let handle = s(empty.as_ptr(), 0);
                    enable_fs(handle);
                    let destroy = sym!(core, "ulDestroyString", unsafe extern "C" fn(Handle));
                    destroy(handle);
                }
            }
        }

        let lib = core;
        let ultra = Self {
            dir: dir.to_path_buf(),
            _core: lib,
            _appcore: appcore,

            create_string: sym!(
                lib,
                "ulCreateStringUTF8",
                unsafe extern "C" fn(*const c_char, usize) -> Handle
            ),
            destroy_string: sym!(lib, "ulDestroyString", unsafe extern "C" fn(Handle)),
            string_data: sym!(
                lib,
                "ulStringGetData",
                unsafe extern "C" fn(Handle) -> *mut c_char
            ),
            string_is_empty: sym!(lib, "ulStringIsEmpty", unsafe extern "C" fn(Handle) -> bool),

            create_config: sym!(lib, "ulCreateConfig", unsafe extern "C" fn() -> Handle),
            destroy_config: sym!(lib, "ulDestroyConfig", unsafe extern "C" fn(Handle)),
            config_set_resource_prefix: sym!(
                lib,
                "ulConfigSetResourcePathPrefix",
                unsafe extern "C" fn(Handle, Handle)
            ),
            config_set_cache_path: sym!(
                lib,
                "ulConfigSetCachePath",
                unsafe extern "C" fn(Handle, Handle)
            ),
            config_set_settle_delay: unsafe {
                lib.get::<unsafe extern "C" fn(Handle, f64)>(b"ulConfigSetPageSettleDelay\0")
                    .ok()
                    .map(|s| *s)
            },
            config_set_min_log_level: unsafe {
                lib.get::<unsafe extern "C" fn(Handle, u32)>(b"ulConfigSetMinLogLevel\0")
                    .ok()
                    .map(|s| *s)
            },

            create_renderer: sym!(
                lib,
                "ulCreateRenderer",
                unsafe extern "C" fn(Handle) -> Handle
            ),
            destroy_renderer: sym!(lib, "ulDestroyRenderer", unsafe extern "C" fn(Handle)),
            update: sym!(lib, "ulUpdate", unsafe extern "C" fn(Handle)),
            refresh_display: sym!(lib, "ulRefreshDisplay", unsafe extern "C" fn(Handle, u32)),
            render: sym!(lib, "ulRender", unsafe extern "C" fn(Handle)),

            create_view_config: sym!(lib, "ulCreateViewConfig", unsafe extern "C" fn() -> Handle),
            destroy_view_config: sym!(lib, "ulDestroyViewConfig", unsafe extern "C" fn(Handle)),
            view_config_set_accelerated: sym!(
                lib,
                "ulViewConfigSetIsAccelerated",
                unsafe extern "C" fn(Handle, bool)
            ),
            view_config_set_javascript: sym!(
                lib,
                "ulViewConfigSetEnableJavaScript",
                unsafe extern "C" fn(Handle, bool)
            ),
            default_session: sym!(
                lib,
                "ulDefaultSession",
                unsafe extern "C" fn(Handle) -> Handle
            ),
            create_view: sym!(
                lib,
                "ulCreateView",
                unsafe extern "C" fn(Handle, u32, u32, Handle, Handle) -> Handle
            ),
            destroy_view: sym!(lib, "ulDestroyView", unsafe extern "C" fn(Handle)),
            view_load_html: sym!(
                lib,
                "ulViewLoadHTMLWithParams",
                unsafe extern "C" fn(Handle, Handle, Handle, bool)
            ),
            view_set_device_scale: sym!(
                lib,
                "ulViewSetDeviceScale",
                unsafe extern "C" fn(Handle, f64)
            ),
            view_is_settled: sym!(lib, "ulViewIsSettled", unsafe extern "C" fn(Handle) -> bool),
            view_evaluate_script: sym!(
                lib,
                "ulViewEvaluateScript",
                unsafe extern "C" fn(Handle, Handle, *mut Handle) -> Handle
            ),
            view_get_surface: sym!(
                lib,
                "ulViewGetSurface",
                unsafe extern "C" fn(Handle) -> Handle
            ),

            surface_width: sym!(
                lib,
                "ulSurfaceGetWidth",
                unsafe extern "C" fn(Handle) -> u32
            ),
            surface_height: sym!(
                lib,
                "ulSurfaceGetHeight",
                unsafe extern "C" fn(Handle) -> u32
            ),
            surface_row_bytes: sym!(
                lib,
                "ulSurfaceGetRowBytes",
                unsafe extern "C" fn(Handle) -> u32
            ),
            surface_lock: sym!(
                lib,
                "ulSurfaceLockPixels",
                unsafe extern "C" fn(Handle) -> *mut c_void
            ),
            surface_unlock: sym!(lib, "ulSurfaceUnlockPixels", unsafe extern "C" fn(Handle)),
        };

        Ok(ultra)
    }

    /// 销毁 Renderer。进程退出前调用；不调也能被系统回收，但会漏掉缓存落盘。
    ///
    /// # Safety
    /// `renderer` 必须来自本实例的 [`Ultra::create_renderer`]，且之后不能再使用。
    pub unsafe fn destroy_renderer(&self, renderer: Handle) {
        unsafe { (self.destroy_renderer)(renderer) };
    }

    /// 创建 Renderer。`resources/` 必须存在，否则 ICU / 证书加载会失败。
    pub fn create_renderer(&self) -> Result<Handle> {
        let resources = self.dir.join("resources");
        if !resources.is_dir() {
            return Err(Error::RuntimeMissing(format!(
                "{} 不存在（Ultralight 需要 icudt*.dat 与 cacert.pem）",
                resources.display()
            )));
        }
        unsafe {
            let config = (self.create_config)();
            if config.is_null() {
                return Err(Error::Render("ulCreateConfig 返回空".into()));
            }

            let prefix = format!("{}/", resources.to_string_lossy());
            let prefix = CString::new(prefix).map_err(|e| Error::Render(e.to_string()))?;
            let prefix = (self.create_string)(prefix.as_ptr(), prefix.as_bytes().len());
            (self.config_set_resource_prefix)(config, prefix);
            (self.destroy_string)(prefix);

            // 缓存目录：留在运行时目录里，卸载 SDK 时一起清掉。
            let cache_dir = self.dir.join("cache");
            let _ = std::fs::create_dir_all(&cache_dir);
            if let Ok(cache) = CString::new(format!("{}/", cache_dir.to_string_lossy())) {
                let cache = (self.create_string)(cache.as_ptr(), cache.as_bytes().len());
                (self.config_set_cache_path)(config, cache);
                (self.destroy_string)(cache);
            }

            if let Some(set) = self.config_set_settle_delay {
                // 默认 0.1s；卡片是纯静态页，调到 0.05 省一点等待。
                set(config, 0.05);
            }
            if let Some(set) = self.config_set_min_log_level {
                // 0=Error，避免 SDK 把 info 日志刷进 stdout 破坏 JSON 协议。
                set(config, 0);
            }

            let renderer = (self.create_renderer)(config);
            (self.destroy_config)(config);
            if renderer.is_null() {
                return Err(Error::Render("ulCreateRenderer 返回空".into()));
            }
            Ok(renderer)
        }
    }

    /// 渲染一段 HTML，返回 RGB 像素与尺寸。
    pub fn render_html(
        &self,
        renderer: Handle,
        html: &str,
        css_width: u32,
        css_height: u32,
        scale: f64,
        timeout_ms: u64,
        ready_expr: Option<&str>,
    ) -> Result<(Vec<u8>, u32, u32)> {
        let pixel_w = ((css_width as f64) * scale).round() as u32;
        let pixel_h = ((css_height as f64) * scale).round() as u32;

        unsafe {
            let view_config = (self.create_view_config)();
            // 走 CPU 光栅化：拿得到像素缓冲，不需要 GPU 上下文。
            (self.view_config_set_accelerated)(view_config, false);
            (self.view_config_set_javascript)(view_config, true);

            let session = (self.default_session)(renderer);
            let view = (self.create_view)(renderer, pixel_w, pixel_h, view_config, session);
            (self.destroy_view_config)(view_config);
            if view.is_null() {
                return Err(Error::Render("ulCreateView 返回空".into()));
            }
            if (scale - 1.0).abs() > f64::EPSILON {
                (self.view_set_device_scale)(view, scale);
            }

            let html_c =
                CString::new(html.replace('\0', "")).map_err(|e| Error::Render(e.to_string()))?;
            let html_s = (self.create_string)(html_c.as_ptr(), html_c.as_bytes().len());
            let url_c = CString::new("about:blank").expect("常量");
            let url_s = (self.create_string)(url_c.as_ptr(), url_c.as_bytes().len());
            (self.view_load_html)(view, html_s, url_s, false);
            (self.destroy_string)(html_s);
            (self.destroy_string)(url_s);

            let started = std::time::Instant::now();
            let mut stable = 0u32;
            loop {
                (self.update)(renderer);
                (self.refresh_display)(renderer, 0);
                (self.render)(renderer);

                if (self.view_is_settled)(view) {
                    let ready = match ready_expr {
                        Some(expr) => self.eval_bool(view, expr).unwrap_or(false),
                        None => true,
                    };
                    if ready {
                        stable += 1;
                        // 连续两帧稳定才算真的完成，避免拿到末帧还没画完的截图。
                        if stable >= 2 {
                            break;
                        }
                    } else {
                        stable = 0;
                    }
                }

                if started.elapsed().as_millis() as u64 > timeout_ms {
                    (self.destroy_view)(view);
                    return Err(Error::Timeout(timeout_ms));
                }
                std::thread::sleep(std::time::Duration::from_millis(2));
            }

            let pixels = self.read_pixels(view)?;
            (self.destroy_view)(view);
            Ok(pixels)
        }
    }

    /// 取 View 的像素，转成 RGB（白底合成）。
    ///
    /// Ultralight 的 CPU surface 是 **BGRA + 预乘 alpha**，所以不能直接丢掉 alpha 通道：
    /// 半透明边缘要按 `c + (255 - a)` 合成到白色背景上，否则文字边缘会发黑。
    // 整个函数体都是 FFI 调用，逐个再包 unsafe 块只会增加噪声。
    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe fn read_pixels(&self, view: Handle) -> Result<(Vec<u8>, u32, u32)> {
        let surface = (self.view_get_surface)(view);
        if surface.is_null() {
            return Err(Error::Render("ulViewGetSurface 返回空".into()));
        }
        let width = (self.surface_width)(surface);
        let height = (self.surface_height)(surface);
        let row_bytes = (self.surface_row_bytes)(surface) as usize;
        if width == 0 || height == 0 {
            return Err(Error::Render("surface 尺寸为 0".into()));
        }

        let data = (self.surface_lock)(surface) as *const u8;
        if data.is_null() {
            return Err(Error::Render("ulSurfaceLockPixels 返回空".into()));
        }

        let mut rgb = vec![0u8; width as usize * height as usize * 3];
        for y in 0..height as usize {
            let row = data.add(y * row_bytes);
            for x in 0..width as usize {
                let px = row.add(x * 4);
                let b = *px as u32;
                let g = *px.add(1) as u32;
                let r = *px.add(2) as u32;
                let a = *px.add(3) as u32;
                let dst = (y * width as usize + x) * 3;
                if a >= 255 {
                    rgb[dst] = r as u8;
                    rgb[dst + 1] = g as u8;
                    rgb[dst + 2] = b as u8;
                } else {
                    let inv = 255 - a;
                    rgb[dst] = (r + inv).min(255) as u8;
                    rgb[dst + 1] = (g + inv).min(255) as u8;
                    rgb[dst + 2] = (b + inv).min(255) as u8;
                }
            }
        }
        (self.surface_unlock)(surface);
        Ok((rgb, width, height))
    }

    /// 在页面里执行一段表达式并判断是否为 `true`。
    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe fn eval_bool(&self, view: Handle, expr: &str) -> Option<bool> {
        let expr_c = CString::new(expr).ok()?;
        let js = (self.create_string)(expr_c.as_ptr(), expr_c.as_bytes().len());
        let mut exception: Handle = std::ptr::null_mut();
        let result = (self.view_evaluate_script)(view, js, &mut exception);
        (self.destroy_string)(js);

        let debug = std::env::var_os("IMAGE_SHOT_DEBUG").is_some();
        // 注意：没有异常时 SDK 返回的是**空的 ULString 对象**，不是空指针。
        // 只看 `!is_null()` 会把每次求值都当成异常，页面永远「不就绪」。
        let has_exception = !exception.is_null() && !(self.string_is_empty)(exception);
        if debug {
            eprintln!(
                "[image-shot] eval {expr:?}: exception={exception:?} empty={} result={result:?}",
                !has_exception
            );
        }
        if has_exception {
            let message = (self.string_data)(exception);
            if !message.is_null() {
                eprintln!(
                    "[image-shot] JS 异常：{}",
                    CStr::from_ptr(message).to_string_lossy()
                );
            }
            return None;
        }
        if result.is_null() {
            return Some(false);
        }
        let data = (self.string_data)(result);
        let value = if data.is_null() {
            String::new()
        } else {
            CStr::from_ptr(data).to_string_lossy().into_owned()
        };
        if debug {
            eprintln!("[image-shot] eval value = {value:?}");
        }
        // 结果字符串归 SDK 所有（和 `ulViewGetURL` 一样），**不能** ulDestroyString，
        // 否则会双释放，进程直接消失。
        Some(value == "true")
    }
}

/// 带「优先用库自身目录找依赖」的加载方式。
unsafe fn load_library(path: &Path) -> Result<&'static Library> {
    #[cfg(windows)]
    {
        use libloading::os::windows::{LOAD_WITH_ALTERED_SEARCH_PATH, Library as WinLibrary};
        let lib = unsafe { WinLibrary::load_with_flags(path, LOAD_WITH_ALTERED_SEARCH_PATH) }
            .map_err(|err| Error::Load(format!("{}: {err}", path.display())))?;
        Ok(Box::leak(Box::new(Library::from(lib))))
    }
    #[cfg(not(windows))]
    {
        let lib = unsafe { Library::new(path) }
            .map_err(|err| Error::Load(format!("{}: {err}", path.display())))?;
        Ok(Box::leak(Box::new(lib)))
    }
}

/// 定位运行时目录。
pub fn find_runtime_dir() -> Option<PathBuf> {
    let dll = if cfg!(windows) {
        "Ultralight.dll"
    } else if cfg!(target_os = "macos") {
        "libUltralight.dylib"
    } else {
        "libUltralight.so"
    };

    let mut candidates = Vec::new();
    if let Ok(dir) = std::env::var("ULTRALIGHT_DIR") {
        candidates.push(PathBuf::from(dir));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        candidates.push(dir.to_path_buf());
        candidates.push(dir.join("ultralight"));
        candidates.push(dir.join("../ultralight"));
        candidates.push(dir.join("../../runtime/ultralight"));
    }
    if let Some(manifest) = option_env!("CARGO_MANIFEST_DIR") {
        candidates.push(Path::new(manifest).join("../../runtime/ultralight"));
    }
    candidates.into_iter().find(|dir| dir.join(dll).exists())
}
