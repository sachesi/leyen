use super::engine::{DepStep, DepStepAction};

#[derive(Clone, Copy)]
pub struct DepProfile {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub category: &'static str,
    pub dependencies: &'static [&'static str],
}

macro_rules! dep {
    ($id:literal, $name:literal, $category:literal) => {
        DepProfile {
            id: $id,
            name: $name,
            description: $name,
            category: $category,
            dependencies: &[],
        }
    };
    ($id:literal, $name:literal, $category:literal, [$($dependency:literal),* $(,)?]) => {
        DepProfile {
            id: $id,
            name: $name,
            description: $name,
            category: $category,
            dependencies: &[$($dependency),*],
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
        "Fonts"
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

pub fn get_dep_steps(id: &str) -> Vec<DepStep> {
    match id {
        "vcredist2022" => vcredist2022_steps(),
        "vcredist2013" => vcredist2013_steps(),
        "vcredist2010" => vcredist2010_steps(),
        "vcredist2008" => vcredist2008_steps(),
        "dotnet48" => dotnet48_steps(),
        "dotnet40" => dotnet40_steps(),
        "dotnet35sp1" => dotnet35sp1_steps(),
        "xna40" => xna40_steps(),
        _ if get_dep_profile(id).is_some() => winetricks_steps(id),
        _ => Vec::new(),
    }
}

fn winetricks_steps(verb: &str) -> Vec<DepStep> {
    vec![DepStep {
        description: "Installing dependency via winetricks…",
        action: DepStepAction::RunWinetricks {
            verb: verb.to_string(),
        },
    }]
}

fn vcredist2022_steps() -> Vec<DepStep> {
    vec![
        DepStep {
            description: "Downloading Visual C++ Redistributable (x86)…",
            action: DepStepAction::DownloadFile {
                url: "https://aka.ms/vs/17/release/vc_redist.x86.exe",
                file_name: "vcredist2022_x86.exe",
            },
        },
        DepStep {
            description: "Installing Visual C++ Redistributable (x86)…",
            action: DepStepAction::RunExe {
                file_name: "vcredist2022_x86.exe",
                args: "/quiet /norestart",
                extra_env: "",
            },
        },
        DepStep {
            description: "Downloading Visual C++ Redistributable (x64)…",
            action: DepStepAction::DownloadFile {
                url: "https://aka.ms/vs/17/release/vc_redist.x64.exe",
                file_name: "vcredist2022_x64.exe",
            },
        },
        DepStep {
            description: "Installing Visual C++ Redistributable (x64)…",
            action: DepStepAction::RunExe {
                file_name: "vcredist2022_x64.exe",
                args: "/quiet /norestart",
                extra_env: "",
            },
        },
        DepStep {
            description: "Configuring Visual C++ DLL overrides…",
            action: DepStepAction::OverrideDlls {
                dlls: "vcruntime140,vcruntime140_1,msvcp140,msvcp140_1,msvcp140_2,concrt140,atl140,vcomp140",
                override_type: "native,builtin",
            },
        },
    ]
}

fn dotnet48_steps() -> Vec<DepStep> {
    vec![
        DepStep {
            description: "Downloading .NET Framework 4.8…",
            action: DepStepAction::DownloadFile {
                url: "https://go.microsoft.com/fwlink/?linkid=2088631",
                file_name: "dotnet48.exe",
            },
        },
        DepStep {
            description: "Installing .NET Framework 4.8…",
            action: DepStepAction::RunExe {
                file_name: "dotnet48.exe",
                args: "/sfxlang:1027 /q /norestart",
                extra_env: "WINEDLLOVERRIDES=fusion=b",
            },
        },
        DepStep {
            description: "Configuring mscoree DLL override…",
            action: DepStepAction::OverrideDlls {
                dlls: "mscoree",
                override_type: "native",
            },
        },
    ]
}

fn vcredist2013_steps() -> Vec<DepStep> {
    vec![
        DepStep {
            description: "Downloading Visual C++ 2013 Redistributable (x86)…",
            action: DepStepAction::DownloadFile {
                url: "https://aka.ms/highdpimfc2013x86enu",
                file_name: "vcredist2013_x86.exe",
            },
        },
        DepStep {
            description: "Installing Visual C++ 2013 Redistributable (x86)…",
            action: DepStepAction::RunExe {
                file_name: "vcredist2013_x86.exe",
                args: "/quiet /norestart",
                extra_env: "",
            },
        },
        DepStep {
            description: "Downloading Visual C++ 2013 Redistributable (x64)…",
            action: DepStepAction::DownloadFile {
                url: "https://aka.ms/highdpimfc2013x64enu",
                file_name: "vcredist2013_x64.exe",
            },
        },
        DepStep {
            description: "Installing Visual C++ 2013 Redistributable (x64)…",
            action: DepStepAction::RunExe {
                file_name: "vcredist2013_x64.exe",
                args: "/quiet /norestart",
                extra_env: "",
            },
        },
        DepStep {
            description: "Configuring Visual C++ 2013 DLL overrides…",
            action: DepStepAction::OverrideDlls {
                dlls: "msvcr120,msvcp120,vccorlib120",
                override_type: "native,builtin",
            },
        },
    ]
}

fn vcredist2010_steps() -> Vec<DepStep> {
    vec![
        DepStep {
            description: "Downloading Visual C++ 2010 SP1 Redistributable (x86)…",
            action: DepStepAction::DownloadFile {
                url: "https://download.microsoft.com/download/1/6/5/165255E7-1014-4D0A-B094-B6A430A6BFFC/vcredist_x86.exe",
                file_name: "vcredist2010_x86.exe",
            },
        },
        DepStep {
            description: "Installing Visual C++ 2010 SP1 Redistributable (x86)…",
            action: DepStepAction::RunExe {
                file_name: "vcredist2010_x86.exe",
                args: "/q /norestart",
                extra_env: "",
            },
        },
        DepStep {
            description: "Downloading Visual C++ 2010 SP1 Redistributable (x64)…",
            action: DepStepAction::DownloadFile {
                url: "https://download.microsoft.com/download/1/6/5/165255E7-1014-4D0A-B094-B6A430A6BFFC/vcredist_x64.exe",
                file_name: "vcredist2010_x64.exe",
            },
        },
        DepStep {
            description: "Installing Visual C++ 2010 SP1 Redistributable (x64)…",
            action: DepStepAction::RunExe {
                file_name: "vcredist2010_x64.exe",
                args: "/q /norestart",
                extra_env: "",
            },
        },
        DepStep {
            description: "Configuring Visual C++ 2010 DLL overrides…",
            action: DepStepAction::OverrideDlls {
                dlls: "msvcr100,msvcp100",
                override_type: "native,builtin",
            },
        },
    ]
}

fn vcredist2008_steps() -> Vec<DepStep> {
    vec![
        DepStep {
            description: "Downloading Visual C++ 2008 SP1 Redistributable (x86)…",
            action: DepStepAction::DownloadFile {
                url: "https://download.microsoft.com/download/5/D/8/5D8C65CB-C849-4025-8E95-C3966CAFD8AE/vcredist_x86.exe",
                file_name: "vcredist2008_x86.exe",
            },
        },
        DepStep {
            description: "Installing Visual C++ 2008 SP1 Redistributable (x86)…",
            action: DepStepAction::RunExe {
                file_name: "vcredist2008_x86.exe",
                args: "/q /norestart",
                extra_env: "",
            },
        },
        DepStep {
            description: "Downloading Visual C++ 2008 SP1 Redistributable (x64)…",
            action: DepStepAction::DownloadFile {
                url: "https://download.microsoft.com/download/5/D/8/5D8C65CB-C849-4025-8E95-C3966CAFD8AE/vcredist_x64.exe",
                file_name: "vcredist2008_x64.exe",
            },
        },
        DepStep {
            description: "Installing Visual C++ 2008 SP1 Redistributable (x64)…",
            action: DepStepAction::RunExe {
                file_name: "vcredist2008_x64.exe",
                args: "/q /norestart",
                extra_env: "",
            },
        },
        DepStep {
            description: "Configuring Visual C++ 2008 DLL overrides…",
            action: DepStepAction::OverrideDlls {
                dlls: "msvcr90,msvcp90",
                override_type: "native,builtin",
            },
        },
    ]
}

fn dotnet40_steps() -> Vec<DepStep> {
    vec![
        DepStep {
            description: "Downloading .NET Framework 4.0…",
            action: DepStepAction::DownloadFile {
                url: "https://download.microsoft.com/download/9/5/A/95A9616B-7A37-4AF6-BC36-D6EA96C8DAAE/dotNetFx40_Full_x86_x64.exe",
                file_name: "dotnet40.exe",
            },
        },
        DepStep {
            description: "Installing .NET Framework 4.0…",
            action: DepStepAction::RunExe {
                file_name: "dotnet40.exe",
                args: "/sfxlang:1027 /q /norestart",
                extra_env: "WINEDLLOVERRIDES=fusion=b",
            },
        },
        DepStep {
            description: "Configuring mscoree DLL override…",
            action: DepStepAction::OverrideDlls {
                dlls: "mscoree",
                override_type: "native",
            },
        },
    ]
}

fn dotnet35sp1_steps() -> Vec<DepStep> {
    vec![
        DepStep {
            description: "Downloading .NET Framework 3.5 SP1…",
            action: DepStepAction::DownloadFile {
                url: "https://download.microsoft.com/download/2/0/E/20E90413-712F-438C-988E-FDAA79A8AC3D/dotnetfx35.exe",
                file_name: "dotnet35.exe",
            },
        },
        DepStep {
            description: "Installing .NET Framework 3.5 SP1…",
            action: DepStepAction::RunExe {
                file_name: "dotnet35.exe",
                args: "/sfxlang:1027 /q /norestart",
                extra_env: "WINEDLLOVERRIDES=fusion=b",
            },
        },
        DepStep {
            description: "Configuring mscoree DLL override…",
            action: DepStepAction::OverrideDlls {
                dlls: "mscoree",
                override_type: "native",
            },
        },
    ]
}

fn xna40_steps() -> Vec<DepStep> {
    vec![
        DepStep {
            description: "Downloading XNA Framework 4.0…",
            action: DepStepAction::DownloadFile {
                url: "https://download.microsoft.com/download/A/C/2/AC2C903B-E6E8-42C2-9FD7-BEBAC362A930/xnafx40_redist.msi",
                file_name: "xnafx40_redist.msi",
            },
        },
        DepStep {
            description: "Installing XNA Framework 4.0…",
            action: DepStepAction::RunMsi {
                file_name: "xnafx40_redist.msi",
                args: "/qn",
            },
        },
    ]
}
