use super::engine::{DepStep, DepStepAction};

// ── Data Structures ──────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct DepCatalogEntry {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub category: &'static str,
}

// ── Built-in catalog ─────────────────────────────────────────────────────────

pub const DEP_CATALOG: &[DepCatalogEntry] = &[
    // ── Runtime ──────────────────────────────────────────────────────────────
    DepCatalogEntry {
        id: "vcredist2022",
        name: "Visual C++ 2015-2022 Redistributable",
        description: "Microsoft Visual C++ runtime libraries required by most modern Windows applications",
        category: "Runtime",
    },
    DepCatalogEntry {
        id: "vcredist2013",
        name: "Visual C++ 2013 Redistributable",
        description: "Microsoft Visual C++ 2013 runtime libraries — required by many older Windows games and apps",
        category: "Runtime",
    },
    DepCatalogEntry {
        id: "vcredist2010",
        name: "Visual C++ 2010 SP1 Redistributable",
        description: "Microsoft Visual C++ 2010 SP1 runtime libraries — required by games built with MSVC 2010",
        category: "Runtime",
    },
    DepCatalogEntry {
        id: "vcredist2008",
        name: "Visual C++ 2008 SP1 Redistributable",
        description: "Microsoft Visual C++ 2008 SP1 runtime libraries — required by legacy Windows software",
        category: "Runtime",
    },
    DepCatalogEntry {
        id: "dotnet48",
        name: ".NET Framework 4.8",
        description: "Microsoft .NET Framework 4.8 — required by many Windows desktop applications",
        category: "Runtime",
    },
    DepCatalogEntry {
        id: "dotnet40",
        name: ".NET Framework 4.0",
        description: "Microsoft .NET Framework 4.0 — required by older .NET applications that predate 4.5+",
        category: "Runtime",
    },
    DepCatalogEntry {
        id: "dotnet35",
        name: ".NET Framework 3.5 SP1",
        description: "Microsoft .NET Framework 3.5 SP1 — required by many older .NET applications and games",
        category: "Runtime",
    },
    DepCatalogEntry {
        id: "xna40",
        name: "XNA Framework 4.0",
        description: "Microsoft XNA Framework 4.0 Redistributable — required to run XNA-based games",
        category: "Runtime",
    },
    // ── DirectX ──────────────────────────────────────────────────────────────
    DepCatalogEntry {
        id: "directx",
        name: "DirectX End-User Runtime (June 2010)",
        description: "Installs legacy DirectX 9/10 components (d3dx9, d3dx10, xinput, etc.) required by older games",
        category: "DirectX",
    },
    DepCatalogEntry {
        id: "d3dcompiler43",
        name: "D3D Compiler 43",
        description: "D3D shader compiler DLL (version 43) — required by some older Direct3D applications and tools",
        category: "DirectX",
    },
    DepCatalogEntry {
        id: "d3dcompiler47",
        name: "D3D Compiler 47",
        description: "D3D shader compiler DLL (version 47) — required by many modern Direct3D applications",
        category: "DirectX",
    },
    // ── Media ─────────────────────────────────────────────────────────────────
    DepCatalogEntry {
        id: "xact",
        name: "XACT Audio",
        description: "Microsoft Cross-Platform Audio Creation Tool runtime — required by many older DirectX games for audio",
        category: "Media",
    },
];

pub const DEP_CATEGORY_ORDER: &[&str] = &["Runtime", "DirectX", "Media"];

pub fn get_dep_steps(id: &str) -> Vec<DepStep> {
    match id {
        "vcredist2022" => vcredist2022_steps(),
        "vcredist2013" => vcredist2013_steps(),
        "vcredist2010" => vcredist2010_steps(),
        "vcredist2008" => vcredist2008_steps(),
        "dotnet48" => dotnet48_steps(),
        "dotnet40" => dotnet40_steps(),
        "dotnet35" => dotnet35_steps(),
        "xna40" => xna40_steps(),
        "directx" => directx_steps(),
        "d3dcompiler43" => d3dcompiler43_steps(),
        "d3dcompiler47" => d3dcompiler47_steps(),
        "xact" => xact_steps(),
        _ => Vec::new(),
    }
}

pub fn get_dep_uninstall_steps(id: &str) -> Vec<DepStep> {
    match id {
        "vcredist2022" => vcredist2022_uninstall_steps(),
        "vcredist2013" => vcredist2013_uninstall_steps(),
        "vcredist2010" => vcredist2010_uninstall_steps(),
        "vcredist2008" => vcredist2008_uninstall_steps(),
        "dotnet48" | "dotnet40" | "dotnet35" => dotnet_uninstall_steps(),
        "directx" => directx_uninstall_steps(),
        "d3dcompiler43" => d3dcompiler43_uninstall_steps(),
        "d3dcompiler47" => d3dcompiler47_uninstall_steps(),
        _ => Vec::new(),
    }
}

// ── Install step functions ────────────────────────────────────────────────────

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

fn dotnet35_steps() -> Vec<DepStep> {
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

fn directx_steps() -> Vec<DepStep> {
    vec![
        DepStep {
            description: "Installing DirectX 9 components (d3dx9_xx)…",
            action: DepStepAction::RunWinetricks { verb: "d3dx9" },
        },
        DepStep {
            description: "Installing DirectX 10 components (d3dx10)…",
            action: DepStepAction::RunWinetricks { verb: "d3dx10" },
        },
        DepStep {
            description: "Installing DirectX 11 components (d3dx11_43)…",
            action: DepStepAction::RunWinetricks { verb: "d3dx11_43" },
        },
        DepStep {
            description: "Configuring d3dx9 DLL overrides…",
            action: DepStepAction::OverrideDlls {
                dlls: "d3dx9_24,d3dx9_25,d3dx9_26,d3dx9_27,d3dx9_28,d3dx9_29,d3dx9_30,\
                       d3dx9_31,d3dx9_32,d3dx9_33,d3dx9_34,d3dx9_35,d3dx9_36,d3dx9_37,\
                       d3dx9_38,d3dx9_39,d3dx9_40,d3dx9_41,d3dx9_42,d3dx9_43",
                override_type: "native,builtin",
            },
        },
    ]
}

fn d3dcompiler43_steps() -> Vec<DepStep> {
    vec![DepStep {
        description: "Installing D3D Compiler 43 via winetricks…",
        action: DepStepAction::RunWinetricks {
            verb: "d3dcompiler_43",
        },
    }]
}

fn d3dcompiler47_steps() -> Vec<DepStep> {
    vec![DepStep {
        description: "Installing D3D Compiler 47 via winetricks…",
        action: DepStepAction::RunWinetricks {
            verb: "d3dcompiler_47",
        },
    }]
}

fn xact_steps() -> Vec<DepStep> {
    vec![
        DepStep {
            description: "Installing XACT audio runtime via winetricks…",
            action: DepStepAction::RunWinetricks { verb: "xact" },
        },
    ]
}

// ── Uninstall step functions ──────────────────────────────────────────────────

fn d3dcompiler43_uninstall_steps() -> Vec<DepStep> {
    vec![
        DepStep {
            description: "Removing D3D Compiler 43 DLL (64-bit)…",
            action: DepStepAction::RemoveDllsFromPrefix {
                dlls: "d3dcompiler_43",
                wine_dir: "system32",
            },
        },
        DepStep {
            description: "Removing D3D Compiler 43 DLL (32-bit)…",
            action: DepStepAction::RemoveDllsFromPrefix {
                dlls: "d3dcompiler_43",
                wine_dir: "syswow64",
            },
        },
        DepStep {
            description: "Removing D3D Compiler 43 DLL override…",
            action: DepStepAction::RemoveDllOverrides {
                dlls: "d3dcompiler_43",
            },
        },
    ]
}

fn d3dcompiler47_uninstall_steps() -> Vec<DepStep> {
    vec![
        DepStep {
            description: "Removing D3D Compiler 47 DLL (64-bit)…",
            action: DepStepAction::RemoveDllsFromPrefix {
                dlls: "d3dcompiler_47",
                wine_dir: "system32",
            },
        },
        DepStep {
            description: "Removing D3D Compiler 47 DLL (32-bit)…",
            action: DepStepAction::RemoveDllsFromPrefix {
                dlls: "d3dcompiler_47",
                wine_dir: "syswow64",
            },
        },
        DepStep {
            description: "Removing D3D Compiler 47 DLL override…",
            action: DepStepAction::RemoveDllOverrides {
                dlls: "d3dcompiler_47",
            },
        },
    ]
}

fn vcredist2022_uninstall_steps() -> Vec<DepStep> {
    vec![DepStep {
        description: "Removing Visual C++ 2022 DLL overrides…",
        action: DepStepAction::RemoveDllOverrides {
            dlls: "vcruntime140,vcruntime140_1,msvcp140,msvcp140_1,msvcp140_2,concrt140,atl140,vcomp140",
        },
    }]
}

fn vcredist2013_uninstall_steps() -> Vec<DepStep> {
    vec![DepStep {
        description: "Removing Visual C++ 2013 DLL overrides…",
        action: DepStepAction::RemoveDllOverrides {
            dlls: "msvcr120,msvcp120,vccorlib120",
        },
    }]
}

fn vcredist2010_uninstall_steps() -> Vec<DepStep> {
    vec![DepStep {
        description: "Removing Visual C++ 2010 DLL overrides…",
        action: DepStepAction::RemoveDllOverrides {
            dlls: "msvcr100,msvcp100",
        },
    }]
}

fn vcredist2008_uninstall_steps() -> Vec<DepStep> {
    vec![DepStep {
        description: "Removing Visual C++ 2008 DLL overrides…",
        action: DepStepAction::RemoveDllOverrides {
            dlls: "msvcr90,msvcp90",
        },
    }]
}

fn dotnet_uninstall_steps() -> Vec<DepStep> {
    vec![DepStep {
        description: "Removing .NET Framework DLL overrides…",
        action: DepStepAction::RemoveDllOverrides { dlls: "mscoree" },
    }]
}

fn directx_uninstall_steps() -> Vec<DepStep> {
    const D3DX9_DLLS: &str =
        "d3dx9_24,d3dx9_25,d3dx9_26,d3dx9_27,d3dx9_28,d3dx9_29,d3dx9_30,\
         d3dx9_31,d3dx9_32,d3dx9_33,d3dx9_34,d3dx9_35,d3dx9_36,d3dx9_37,\
         d3dx9_38,d3dx9_39,d3dx9_40,d3dx9_41,d3dx9_42,d3dx9_43";

    const D3DCOMP_DLLS: &str =
        "d3dcompiler_33,d3dcompiler_34,d3dcompiler_35,d3dcompiler_36,\
         d3dcompiler_37,d3dcompiler_38,d3dcompiler_39,d3dcompiler_40,\
         d3dcompiler_41,d3dcompiler_42,d3dcompiler_43,d3dcompiler_46";

    const D3DX10_DLLS: &str =
        "d3dx10,d3dx10_33,d3dx10_34,d3dx10_35,d3dx10_36,d3dx10_37,\
         d3dx10_38,d3dx10_39,d3dx10_40,d3dx10_41,d3dx10_42,d3dx10_43";

    const D3DX11_DLLS: &str = "d3dx11_43";

    vec![
        DepStep {
            description: "Removing d3dx9 DLL files (64-bit)…",
            action: DepStepAction::RemoveDllsFromPrefix {
                dlls: D3DX9_DLLS,
                wine_dir: "system32",
            },
        },
        DepStep {
            description: "Removing d3dcompiler DLL files (64-bit)…",
            action: DepStepAction::RemoveDllsFromPrefix {
                dlls: D3DCOMP_DLLS,
                wine_dir: "system32",
            },
        },
        DepStep {
            description: "Removing d3dx10 DLL files (64-bit)…",
            action: DepStepAction::RemoveDllsFromPrefix {
                dlls: D3DX10_DLLS,
                wine_dir: "system32",
            },
        },
        DepStep {
            description: "Removing d3dx11 DLL files (64-bit)…",
            action: DepStepAction::RemoveDllsFromPrefix {
                dlls: D3DX11_DLLS,
                wine_dir: "system32",
            },
        },
        DepStep {
            description: "Removing d3dx9 DLL files (32-bit)…",
            action: DepStepAction::RemoveDllsFromPrefix {
                dlls: D3DX9_DLLS,
                wine_dir: "syswow64",
            },
        },
        DepStep {
            description: "Removing d3dcompiler DLL files (32-bit)…",
            action: DepStepAction::RemoveDllsFromPrefix {
                dlls: D3DCOMP_DLLS,
                wine_dir: "syswow64",
            },
        },
        DepStep {
            description: "Removing d3dx10 DLL files (32-bit)…",
            action: DepStepAction::RemoveDllsFromPrefix {
                dlls: D3DX10_DLLS,
                wine_dir: "syswow64",
            },
        },
        DepStep {
            description: "Removing d3dx11 DLL files (32-bit)…",
            action: DepStepAction::RemoveDllsFromPrefix {
                dlls: D3DX11_DLLS,
                wine_dir: "syswow64",
            },
        },
        DepStep {
            description: "Removing DirectX DLL overrides…",
            action: DepStepAction::RemoveDllOverrides { dlls: D3DX9_DLLS },
        },
    ]
}
