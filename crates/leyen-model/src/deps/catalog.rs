#[derive(Clone, Copy)]
pub struct DepProfile {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub category: &'static str,
    pub dependencies: &'static [&'static str],
    pub provides: &'static [&'static str],
}

macro_rules! dep {
    ($id:literal, $name:literal, $category:literal) => {
        DepProfile {
            id: $id,
            name: $name,
            description: $name,
            category: $category,
            dependencies: &[],
            provides: &[],
        }
    };
    ($id:literal, $name:literal, $category:literal, [$($dependency:literal),* $(,)?]) => {
        DepProfile {
            id: $id,
            name: $name,
            description: $name,
            category: $category,
            dependencies: &[$($dependency),*],
            provides: &[],
        }
    };
    ($id:literal, $name:literal, $category:literal, [$($dependency:literal),* $(,)?], provides: [$($provides:literal),* $(,)?] $(,)?) => {
        DepProfile {
            id: $id,
            name: $name,
            description: $name,
            category: $category,
            dependencies: &[$($dependency),*],
            provides: &[$($provides),*],
        }
    };
}

pub const DEP_PROFILES: &[DepProfile] = &[
    dep!("mono", "Wine mono", "Wine"),
    dep!("gecko", "Wine gecko", "Wine"),
    dep!(
        "vbrun6",
        "Microsoft Visual Basic 6 Runtime SP6",
        "Redistributables"
    ),
    dep!(
        "vcredist6",
        "Microsoft Visual C++ 6 SP4 libraries",
        "Redistributables"
    ),
    dep!(
        "vcredist6sp6",
        "Microsoft Visual C++ 6 SP6 libraries",
        "Redistributables"
    ),
    dep!(
        "vcredist2005",
        "Microsoft Visual C++ Redistributable for Visual Studio 2005",
        "Redistributables"
    ),
    dep!(
        "vcredist2008",
        "Microsoft Visual C++ Redistributable for Visual Studio 2008",
        "Redistributables"
    ),
    dep!(
        "vcredist2010",
        "Microsoft Visual C++ Redistributable for Visual Studio 2010",
        "Redistributables"
    ),
    dep!(
        "vcredist2012",
        "Microsoft Visual C++ Redistributable for Visual Studio 2012",
        "Redistributables"
    ),
    dep!(
        "vcredist2013",
        "Microsoft Visual C++ Redistributable (2013)",
        "Redistributables"
    ),
    dep!(
        "vcredist2015",
        "Microsoft Visual C++ Redistributable (2015)",
        "Redistributables"
    ),
    dep!(
        "vcredist2019",
        "Microsoft Visual C++ Redistributable (2015-2019)",
        "Redistributables"
    ),
    dep!(
        "vcredist2022",
        "Microsoft Visual C++ Redistributable (2015-2022)",
        "Redistributables"
    ),
    dep!("dotnet20", "Microsoft .NET Framework 2.0", ".NET"),
    dep!(
        "dotnet20sp1",
        "Microsoft .NET Framework 2.0 Service Pack 1",
        ".NET",
        ["dotnet20"]
    ),
    dep!("dotnet35", "Microsoft .NET Framework 3.5", ".NET"),
    dep!(
        "dotnet35sp1",
        "Microsoft .NET Framework 3.5 Service Pack 1",
        ".NET",
        ["dotnet35"]
    ),
    dep!("dotnet40", "Microsoft .NET Framework 4", ".NET"),
    dep!("dotnet45", "Microsoft .NET Framework 4.5", ".NET"),
    dep!("dotnet452", "Microsoft .NET Framework 4.5.2", ".NET"),
    dep!("dotnet46", "Microsoft .NET Framework 4.6", ".NET"),
    dep!("dotnet461", "Microsoft .NET Framework 4.6.1", ".NET"),
    dep!("dotnet462", "Microsoft .NET Framework 4.6.2", ".NET"),
    dep!("dotnet472", "Microsoft .NET Framework 4.7.2", ".NET"),
    dep!(
        "dotnet48",
        "Microsoft .NET Framework 4.8",
        ".NET",
        ["dotnet40"]
    ),
    dep!("dotnet481", "Microsoft .NET Framework 4.8.1", ".NET"),
    dep!("dotnetcore3", "Microsoft .NET Core Runtime 3.1 LTS", ".NET"),
    dep!(
        "dotnetcoredesktop3",
        "Microsoft .NET Core Desktop Runtime 3.1 LTS",
        ".NET"
    ),
    dep!(
        "dotnetcoredesktop6",
        "Microsoft .NET Core Desktop Runtime 6.0 LTS",
        ".NET"
    ),
    dep!(
        "dotnetcoredesktop7",
        "Microsoft .NET Core Desktop Runtime 7.0",
        ".NET"
    ),
    dep!(
        "dotnetcoredesktop8",
        "Microsoft .NET Core Desktop Runtime 8.0 LTS",
        ".NET"
    ),
    dep!(
        "dotnetcoredesktop9",
        "Microsoft .NET Core Desktop Runtime 9.0",
        ".NET"
    ),
    dep!(
        "dotnetcoredesktop10",
        "Microsoft .NET Core Desktop Runtime 10.0 LTS",
        ".NET"
    ),
    dep!("sqlite3", "SQLite3", "Generic"),
    dep!("winhttp", "Microsoft Windows HTTP Services", "Generic"),
    dep!("wininet", "Windows Internet API", "Generic"),
    dep!("urlmon", "Uniform Resource Locator Moniker", "Generic"),
    dep!(
        "iertutil",
        "Internet Explorer Run-Time Utility Library",
        "Generic"
    ),
    dep!("aairruntime", "Harman AIR runtime", "Generic"),
    dep!("gfw", "MS Games For Windows Live (xlive.dll)", "Generic"),
    dep!("gmdls", "General MIDI DLS Collection", "Generic"),
    dep!(
        "mdac28",
        "Microsoft Data Access Components 2.8 SP1",
        "Generic"
    ),
    dep!(
        "mfc40",
        "Microsoft mfc40 Microsoft Foundation Classes",
        "Generic"
    ),
    dep!(
        "mfc42",
        "Microsoft mfc42 Microsoft Foundation Classes",
        "Generic"
    ),
    dep!("msasn1", "MS ASN1", "Generic"),
    dep!("mspatcha", "Microsoft mspatcha.dll", "Generic"),
    dep!(
        "msxml3",
        "Microsoft Core XML Services (MSXML) 3.0",
        "Generic"
    ),
    dep!(
        "msxml4",
        "Microsoft Core XML Services (MSXML) 4.0",
        "Generic"
    ),
    dep!(
        "msxml6",
        "Microsoft Core XML Services (MSXML) 6.0",
        "Generic"
    ),
    dep!("mediafoundation", "Microsoft Media Foundation", "Generic"),
    dep!("jet40", "MS Jet 4.0 Service Pack 8", "Generic"),
    dep!("art2kmin", "MS Access 2000 runtime", "Generic"),
    dep!("art2k7min", "MS Access 2007 runtime", "Generic"),
    dep!(
        "riched20",
        "Microsoft RichEdit Control 2.0 (riched20.dll)",
        "Generic"
    ),
    dep!(
        "msftedit",
        "Microsoft RichEdit Control 4.1 (msftedit.dll)",
        "Generic"
    ),
    dep!("msls31", "Microsoft Line Services", "Generic"),
    dep!(
        "gdiplus",
        "Microsoft GDI+ (Graphics Device Interface)",
        "Generic"
    ),
    dep!("atmlib", "Adobe Type Manager", "Generic"),
    dep!("physx", "NVIDIA PhysX System 9.19.0218", "Generic"),
    dep!("quicktime72", "QuickTime 7.2.0.240", "Generic"),
    dep!("xact", "MS XACT Engine (32-bit only)", "Generic"),
    dep!("xact_x64", "MS XACT Engine (64-bit only)", "Generic"),
    dep!(
        "xinput",
        "Microsoft XInput (Xbox controller support)",
        "Generic"
    ),
    dep!(
        "ie8_kb2936068",
        "Cumulative Security Update for Internet Explorer 8",
        "Generic"
    ),
    dep!("wsh57", "MS Windows Script Host 5.7", "Generic"),
    dep!("webview2", "Microsoft Edge Web View 2", "Generic"),
    dep!(
        "powershell",
        "Windows PowerShell Wrapper For Wine",
        "Generic"
    ),
    dep!("powershell_core", "Microsoft PowerShell Core", "Generic"),
    dep!(
        "d3dx9",
        "Microsoft d3dx9 DLLs from DirectX 9 redistributable",
        "Direct3D"
    ),
    dep!("d3dcompiler_42", "Microsoft d3dcompiler_42.dll", "Direct3D"),
    dep!("d3dcompiler_43", "Microsoft d3dcompiler_43.dll", "Direct3D"),
    dep!("d3dcompiler_46", "Microsoft d3dcompiler_46.dll", "Direct3D"),
    dep!("d3dcompiler_47", "Microsoft d3dcompiler_47.dll", "Direct3D"),
    dep!(
        "d3dx11",
        "Microsoft d3dx11 DLLs from DirectX 11 redistributable",
        "Direct3D"
    ),
    dep!(
        "cnc-ddraw",
        "Re-implementation of the DirectDraw API for classic games",
        "Direct3D"
    ),
    dep!(
        "dx8vb",
        "Microsoft dx8vb.dll from DirectX 8.1 runtime",
        "Direct3D"
    ),
    dep!("amstream", "Microsoft amstream.dll", "DirectX Media"),
    dep!("devenum", "Microsoft devenum.dll", "DirectX Media"),
    dep!(
        "directplay",
        "Microsoft DirectPlay redistributable",
        "DirectX Media"
    ),
    dep!(
        "directmusic",
        "All Microsoft DirectMusic dependencies",
        "DirectX Media"
    ),
    dep!(
        "directshow",
        "All Microsoft DirectShow dependencies",
        "DirectX Media"
    ),
    dep!("dmband", "Microsoft dmband.dll", "DirectX Media"),
    dep!("dmcompos", "Microsoft dmcompos.dll", "DirectX Media"),
    dep!("dmime", "Microsoft dmime.dll", "DirectX Media"),
    dep!("dmloader", "Microsoft dmloader.dll", "DirectX Media"),
    dep!("dmscript", "Microsoft dmscript.dll", "DirectX Media"),
    dep!("dmstyle", "Microsoft dmstyle.dll", "DirectX Media"),
    dep!("dmsynth", "Microsoft dmsynth.dll", "DirectX Media"),
    dep!("dmusic", "Microsoft dmusic.dll", "DirectX Media"),
    dep!("dmusic32", "Microsoft dmusic32.dll", "DirectX Media"),
    dep!("dsound", "Microsoft dsound.dll", "DirectX Media"),
    dep!("dswave", "Microsoft dswave.dll", "DirectX Media"),
    dep!("dsdmo", "Microsoft dsdmo.dll", "DirectX Media"),
    dep!("qasf", "Microsoft qasf.dll", "DirectX Media"),
    dep!("qcap", "Microsoft qcap.dll", "DirectX Media"),
    dep!("qdvd", "Microsoft qdvd.dll", "DirectX Media"),
    dep!("qedit", "Microsoft qedit.dll", "DirectX Media"),
    dep!("quartz", "Microsoft quartz.dll", "DirectX Media"),
    dep!("xna31", "Microsoft XNA Redistributable 3.1", "XNA"),
    dep!("xna40", "Microsoft XNA Redistributable 4.0", "XNA"),
    dep!("ffdshow", "ffdshow video codecs", "Codecs"),
    dep!("dirac", "The Dirac directshow filter v1.0.2", "Codecs"),
    dep!(
        "l3codecx",
        "MPEG Layer-3 Audio Codec for Microsoft DirectShow",
        "Codecs"
    ),
    dep!("lavfilters702", "LAV Filters 0.70.2", "Codecs"),
    dep!("lavfilters741", "LAV Filters 0.74.1", "Codecs"),
    dep!(
        "unifont",
        "Unifont replacement for Arial Unicode MS",
        "Fonts"
    ),
    dep!(
        "allfonts",
        "All Microsoft and Adobe essential fonts",
        "Fonts",
        [],
        provides: [
            "arial32", "arialb32", "andale32", "comic32", "courie32",
            "georgi32", "impact32", "times32", "tahoma32", "trebuc32",
            "verdan32", "webdin32",
        ],
    ),
    dep!("cjkfonts", "All Chinese/Japanese/Korean fonts", "Fonts"),
    dep!("arial32", "Microsoft Arial Font", "Fonts"),
    dep!("arialb32", "Microsoft Arial Black Font", "Fonts"),
    dep!("andale32", "Microsoft Andale Font", "Fonts"),
    dep!("comic32", "Microsoft Comic Sans MS Font", "Fonts"),
    dep!("courie32", "Microsoft Courier New Font", "Fonts"),
    dep!("georgi32", "Microsoft Georgia Font", "Fonts"),
    dep!("impact32", "Microsoft Impact Font", "Fonts"),
    dep!("times32", "Microsoft Times New Roman Font", "Fonts"),
    dep!("tahoma32", "Microsoft Tahoma Font", "Fonts"),
    dep!("trebuc32", "Microsoft Trebuchet Font", "Fonts"),
    dep!("verdan32", "Microsoft Verdan Font", "Fonts"),
    dep!("webdin32", "Webdings Font", "Fonts"),
    dep!("consolas", "MS Consolas console font", "Fonts"),
    dep!("lucon", "MS Lucida console font", "Fonts"),
];

pub const DEP_CATEGORY_ORDER: &[&str] = &[
    "Wine",
    "Redistributables",
    ".NET",
    "Generic",
    "Direct3D",
    "DirectX Media",
    "XNA",
    "Codecs",
    "Fonts",
];

pub fn get_dep_profile(id: &str) -> Option<&'static DepProfile> {
    DEP_PROFILES.iter().find(|profile| profile.id == id)
}
