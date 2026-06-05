//! Per-dependency install step recipes (the `DepStep` programs executed by the
//! engine). Metadata (`DepProfile`, `DEP_PROFILES`) lives in `leyen-model`.

use crate::deps::engine::{DepStep, DepStepAction, VerifyAction};
use leyen_model::deps::get_dep_profile;

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
                sha256: "0c09f2611660441084ce0df425c51c11e147e6447963c3690f97e0b25c55ed64",
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
                sha256: "cc0ff0eb1dc3f5188ae6300faef32bf5beeba4bdd6e8e445a9184072096b713b",
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
            },
        },
        DepStep {
            description: "Verifying Visual C++ 2015-2022 installation…",
            action: DepStepAction::Verify {
                description: "VCRedist 2022 x64 Registry Key",
                action: VerifyAction::RegistryKeyExists {
                    path: "HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\VisualStudio\\14.0\\VC\\Runtimes\\x64",
                },
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
                sha256: "0a3a390c47e639d0f7fc65b21195fee6b7f65b066f80f70c60fab191d14b7e40",
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
                sha256: "53b605d1100ab0a88b867447bbf9274b5938125024ba01f5105a9e178a3dcdbd",
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
                sha256: "a4bba7701e355ae29c403431f871a537897c363e215cafe706615e270984f17c",
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
                sha256: "99dce3c841cc6028560830f7866c9ce2928c98cf3256892ef8e6cf755147b0d8",
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
                sha256: "f3b7a76d84d23f91957aa18456a14b4e90609e4ce8194c5653384ed38dada6f3",
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
                sha256: "8742bcbf24ef328a72d2a27b693cc7071e38d3bb4b9b44dec42aa3d2c8d61d92",
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
                sha256: "c5e273a4a16ab4d5471e91c7477719a2f45ddadb76c7f98a38fa5074a6838654",
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
                sha256: "65e064258f2e418816b304f646ff9e87af101e4c9552ab064bb74d281c38659f",
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
                sha256: "0582515bde321e072f8673e829e175ed2e7a53e803127c50253af76528e66bc1",
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
                sha256: "47260420773a20443fb1e38a89b7f39c5237a80a842de598e6c8f7f90a3bbd6d",
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
