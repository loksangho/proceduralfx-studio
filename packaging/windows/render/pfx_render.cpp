// pfx-render.exe: runs MoonRay (or its Hydra renderer, hd_render) for ProceduralFX
// Studio on Windows, inside a Linux container through the WSL Containers SDK.
//
//   pfx-render.exe --check           exit 0 if WSL containers are ready, else 3
//   pfx-render.exe --install         install what's missing (may ask to restart)
//   pfx-render.exe --pull            download the MoonRay image if it isn't here yet
//   pfx-render.exe --diagnose        try containers step by step (for troubleshooting)
//   pfx-render.exe moonray <args>    render: the arguments go to moonray as they are,
//   pfx-render.exe hd_render <args>  with Windows paths mapped into the container
//
// Every local drive is visible in the container under /mnt/host/<letter>, so
// C:\Users\me\scene.rdla is /mnt/host/c/Users/me/scene.rdla. The app writes the
// paths inside its scene files (textures, HDRs) with the same rule.
//
// The image (ghcr.io/loksangho/pfx-moonray, built from packaging/windows/moonray) is
// downloaded the first time and kept in the session's storage. Closing this program's
// standard input stops the render (that's how the app cancels).
//
// Progress for the app is on stderr, in lines starting "[pfx-render]":
//   [pfx-render] download <bytes done> <bytes total>
//   [pfx-render] status <text>
//   [pfx-render] error <text>
//
// Based on Microsoft's WSLC MoonRay sample (MIT licence):
// github.com/microsoft/Build26-DEM346-whats-new-in-windows-subsystem-for-linux

#include <winsock2.h>
#include <windows.h>
#include <objbase.h>
#include <shellapi.h>
#include <shlobj.h>
#include <stdio.h>

#include <map>
#include <mutex>
#include <string>
#include <vector>

#include "wslcsdk.h"

#ifndef PFX_MOONRAY_IMAGE
#define PFX_MOONRAY_IMAGE "ghcr.io/loksangho/pfx-moonray:latest"
#endif

// Exit codes the app understands (MoonRay's own codes pass through otherwise).
static const int EXIT_NOT_READY = 3;    // WSL containers aren't installed
static const int EXIT_SETUP_FAILED = 4; // the session, image or container failed
static const int EXIT_NEEDS_ADMIN = 5;  // installing needs an administrator, who declined

// What an administrator can run once instead (it also turns on Virtual Machine Platform).
static const wchar_t* ADMIN_COMMAND = L"wsl --install --no-distribution";

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

static void Status(const wchar_t* text)
{
    fwprintf(stderr, L"[pfx-render] status %s\n", text);
    fflush(stderr);
}

static void Fail(const wchar_t* context, HRESULT hr, PWSTR error)
{
    fwprintf(stderr, L"[pfx-render] error %s (0x%08X)%s%s\n", context, (unsigned)hr, error ? L": " : L"", error ? error : L"");
    fflush(stderr);
    if (error)
        CoTaskMemFree(error);
}

static std::string Utf8(const std::wstring& wide)
{
    if (wide.empty())
        return std::string();
    int len = WideCharToMultiByte(CP_UTF8, 0, wide.c_str(), -1, nullptr, 0, nullptr, nullptr);
    std::string s(len - 1, '\0');
    WideCharToMultiByte(CP_UTF8, 0, wide.c_str(), -1, &s[0], len, nullptr, nullptr);
    return s;
}

// ---------------------------------------------------------------------------
// Paths: X:\dir\file → /mnt/host/x/dir/file
// ---------------------------------------------------------------------------

static bool IsDrivePath(const std::wstring& s)
{
    return s.size() >= 3 && iswalpha(s[0]) && s[1] == L':' && (s[2] == L'\\' || s[2] == L'/');
}

static std::string ContainerPath(const std::wstring& windowsPath)
{
    std::wstring rest = windowsPath.substr(2);
    for (auto& c : rest)
        if (c == L'\\')
            c = L'/';
    std::string out = "/mnt/host/";
    out += (char)towlower(windowsPath[0]);
    out += Utf8(rest);
    return out;
}

// An argument mapped for the container: a Windows path, or "-flag=C:\path", becomes
// its container path; anything else is passed as it is.
static std::string MapArgument(const std::wstring& arg)
{
    if (IsDrivePath(arg))
        return ContainerPath(arg);
    size_t eq = arg.find(L'=');
    if (eq != std::wstring::npos && IsDrivePath(arg.substr(eq + 1)))
        return Utf8(arg.substr(0, eq + 1)) + ContainerPath(arg.substr(eq + 1));
    // A relative path to an existing file: make it absolute first.
    if (arg.find(L'\\') != std::wstring::npos && GetFileAttributesW(arg.c_str()) != INVALID_FILE_ATTRIBUTES) {
        wchar_t full[32768];
        if (GetFullPathNameW(arg.c_str(), 32768, full, nullptr) && IsDrivePath(full))
            return ContainerPath(full);
    }
    return Utf8(arg);
}

struct DriveMount {
    std::wstring windowsPath; // "C:\"
    std::string containerPath; // "/mnt/host/c"
};

// The local and removable drives (network drives aren't visible to the VM).
static std::vector<DriveMount> Drives()
{
    std::vector<DriveMount> out;
    DWORD mask = GetLogicalDrives();
    for (int i = 0; i < 26; i++) {
        if (!(mask & (1u << i)))
            continue;
        wchar_t root[4] = { (wchar_t)(L'A' + i), L':', L'\\', 0 };
        UINT type = GetDriveTypeW(root);
        if (type != DRIVE_FIXED && type != DRIVE_REMOVABLE)
            continue;
        DriveMount m;
        m.windowsPath = root;
        m.containerPath = std::string("/mnt/host/") + (char)('a' + i);
        out.push_back(m);
    }
    return out;
}

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

static std::wstring StoragePath()
{
    PWSTR local = nullptr;
    std::wstring path = L"C:\\ProgramData\\ProceduralFX Studio\\wslc";
    if (SUCCEEDED(SHGetKnownFolderPath(FOLDERID_LocalAppData, 0, nullptr, &local))) {
        path = std::wstring(local) + L"\\ProceduralFX Studio\\wslc";
        CoTaskMemFree(local);
    }
    SHCreateDirectoryExW(nullptr, path.c_str(), nullptr);
    return path;
}

static HRESULT OpenSession(WslcSession* session)
{
    WslcSessionSettings settings;
    std::wstring storage = StoragePath();
    HRESULT hr = WslcInitSessionSettings(L"ProceduralFXStudio", storage.c_str(), &settings);
    if (FAILED(hr)) {
        Fail(L"couldn't set up the container session", hr, nullptr);
        return hr;
    }
    // Every CPU, and three quarters of the memory (4 GB to 64 GB).
    SYSTEM_INFO sys;
    GetSystemInfo(&sys);
    WslcSetSessionSettingsCpuCount(&settings, sys.dwNumberOfProcessors);
    MEMORYSTATUSEX mem = { sizeof(mem) };
    uint64_t totalMB = GlobalMemoryStatusEx(&mem) ? mem.ullTotalPhys / (1024 * 1024) : 8192;
    uint64_t useMB = totalMB * 3 / 4;
    if (useMB < 4096)
        useMB = 4096;
    if (useMB > 65536)
        useMB = 65536;
    WslcSetSessionSettingsMemory(&settings, (uint32_t)useMB);
    // The image is several GB once unpacked; the disk only grows as it's used.
    WslcVhdRequirements vhd = {};
    vhd.sizeBytes = 48ULL * 1024 * 1024 * 1024;
    vhd.type = WSLC_VHD_TYPE_DYNAMIC;
    WslcSetSessionSettingsVhd(&settings, &vhd);

    PWSTR error = nullptr;
    hr = WslcCreateSession(&settings, session, &error);
    if (FAILED(hr))
        Fail(L"couldn't start the container session", hr, error);
    return hr;
}

// ---------------------------------------------------------------------------
// The image
// ---------------------------------------------------------------------------

// Image names as the session lists them may leave out the registry ("ghcr.io/").
static bool SameImage(const char* listed, const char* wanted)
{
    if (strcmp(listed, wanted) == 0)
        return true;
    const char* slash = strchr(wanted, '/');
    return slash && strcmp(listed, slash + 1) == 0;
}

static bool HasImage(WslcSession session, const char* image)
{
    WslcImageInfo* images = nullptr;
    uint32_t count = 0;
    bool found = false;
    if (SUCCEEDED(WslcListSessionImages(session, &images, &count))) {
        for (uint32_t i = 0; i < count; i++)
            found = found || SameImage(images[i].name, image);
        if (images)
            CoTaskMemFree(images);
    }
    return found;
}

// Older copies of the image (from earlier versions of the app) take disk space.
static void RemoveOldImages(WslcSession session, const char* image)
{
    std::string base(image);
    base = base.substr(0, base.rfind(':'));
    std::string shortBase = base.find('/') != std::string::npos ? base.substr(base.find('/') + 1) : base;
    WslcImageInfo* images = nullptr;
    uint32_t count = 0;
    if (FAILED(WslcListSessionImages(session, &images, &count)))
        return;
    std::vector<std::string> old;
    for (uint32_t i = 0; i < count; i++) {
        std::string name = images[i].name;
        bool ours = name.rfind(base + ":", 0) == 0 || name.rfind(shortBase + ":", 0) == 0;
        if (ours && !SameImage(images[i].name, image))
            old.push_back(name);
    }
    if (images)
        CoTaskMemFree(images);
    for (auto& name : old) {
        PWSTR error = nullptr;
        if (SUCCEEDED(WslcDeleteSessionImage(session, name.c_str(), &error)))
            fwprintf(stderr, L"[pfx-render] status removed the older image %hs\n", name.c_str());
        else if (error)
            CoTaskMemFree(error);
    }
}

// Download progress, added up over the image's layers.
struct PullProgress {
    std::mutex lock;
    std::map<std::string, std::pair<uint64_t, uint64_t>> layers;
    ULONGLONG lastPrint = 0;
};

static HRESULT CALLBACK OnPullProgress(const WslcImageProgressMessage* message, PVOID context)
{
    auto* progress = static_cast<PullProgress*>(context);
    if (!message || !progress)
        return S_OK;
    std::lock_guard<std::mutex> guard(progress->lock);
    if (message->id && message->detail.totalBytes > 0)
        progress->layers[message->id] = { message->detail.currentBytes, message->detail.totalBytes };
    ULONGLONG now = GetTickCount64();
    if (now - progress->lastPrint < 500)
        return S_OK;
    progress->lastPrint = now;
    uint64_t done = 0, total = 0;
    for (auto& layer : progress->layers) {
        done += layer.second.first;
        total += layer.second.second;
    }
    fwprintf(stderr, L"[pfx-render] download %llu %llu\n", (unsigned long long)done, (unsigned long long)total);
    fflush(stderr);
    return S_OK;
}

static HRESULT EnsureImage(WslcSession session, const char* image)
{
    if (HasImage(session, image))
        return S_OK;
    fwprintf(stderr, L"[pfx-render] status downloading MoonRay (%hs); this happens once\n", image);
    fflush(stderr);
    PullProgress progress;
    WslcPullImageOptions options = {};
    options.uri = image;
    options.progressCallback = OnPullProgress;
    options.progressCallbackContext = &progress;
    PWSTR error = nullptr;
    HRESULT hr = WslcPullSessionImage(session, &options, &error);
    if (FAILED(hr)) {
        Fail(L"couldn't download the MoonRay image (check your internet connection)", hr, error);
        return hr;
    }
    RemoveOldImages(session, image);
    return S_OK;
}

// ---------------------------------------------------------------------------
// Running
// ---------------------------------------------------------------------------

static void CALLBACK OnOutput(WslcProcessIOHandle handle, const BYTE* data, uint32_t bytes, PVOID)
{
    HANDLE out = GetStdHandle(handle == WSLC_PROCESS_IO_HANDLE_STDOUT ? STD_OUTPUT_HANDLE : STD_ERROR_HANDLE);
    DWORD written;
    WriteFile(out, data, bytes, &written, nullptr);
}

struct Exit {
    HANDLE event;
    INT32 code;
};

static void CALLBACK OnExit(INT32 code, PVOID context)
{
    auto* exit = static_cast<Exit*>(context);
    exit->code = code;
    SetEvent(exit->event);
}

// Set when standard input closes: the app cancelled the render.
static HANDLE g_cancelled = nullptr;

static DWORD WINAPI WatchInput(LPVOID)
{
    HANDLE in = GetStdHandle(STD_INPUT_HANDLE);
    char buffer[256];
    DWORD read = 0;
    // Only a pipe can be watched this way (a console would wait for typing).
    if (in == INVALID_HANDLE_VALUE || in == nullptr || GetFileType(in) != FILE_TYPE_PIPE)
        return 0;
    while (ReadFile(in, buffer, sizeof(buffer), &read, nullptr) && read > 0) {
    }
    SetEvent(g_cancelled);
    return 0;
}

// What a container's first process printed and how it ended (for diagnosing a
// container that stops straight away).
struct InitWatch {
    std::mutex lock;
    std::string output;
    INT32 code = -1;
    bool exited = false;
};

static void CALLBACK OnInitOutput(WslcProcessIOHandle, const BYTE* data, uint32_t bytes, PVOID context)
{
    auto* watch = static_cast<InitWatch*>(context);
    std::lock_guard<std::mutex> guard(watch->lock);
    if (watch->output.size() < 4096)
        watch->output.append(reinterpret_cast<const char*>(data), bytes);
}

static void CALLBACK OnInitExit(INT32 code, PVOID context)
{
    auto* watch = static_cast<InitWatch*>(context);
    std::lock_guard<std::mutex> guard(watch->lock);
    watch->code = code;
    watch->exited = true;
}

// Explains a container that isn't running: its first process's output and exit code,
// and the container's state as WSL describes it.
static void ExplainStopped(WslcContainer container, InitWatch* watch)
{
    {
        std::lock_guard<std::mutex> guard(watch->lock);
        if (watch->exited)
            fwprintf(stderr, L"[pfx-render] status the container's first process exited with %d\n", watch->code);
        if (!watch->output.empty())
            fwprintf(stderr, L"[pfx-render] status it printed: %hs\n", watch->output.c_str());
    }
    PSTR inspect = nullptr;
    if (container && SUCCEEDED(WslcInspectContainer(container, &inspect)) && inspect) {
        std::string text(inspect);
        CoTaskMemFree(inspect);
        // The part about its state is what matters.
        size_t at = text.find("\"State\"");
        text = at != std::string::npos ? text.substr(at, 600) : text.substr(0, 600);
        fwprintf(stderr, L"[pfx-render] status container: %hs\n", text.c_str());
    }
    fflush(stderr);
}

// Creates and starts a container of `image` whose first process is `initArgv` (it
// keeps the container running), sharing the local drives when `shareDrives`. Waits a
// moment and checks it's still running; explains it if not.
static HRESULT StartContainer(WslcSession session, const char* image, bool shareDrives, bool autoRemove, std::vector<PCSTR> initArgv, WslcContainer* container, InitWatch* watch)
{
    PWSTR error = nullptr;
    WslcProcessSettings init;
    WslcInitProcessSettings(&init);
    WslcSetProcessSettingsCmdLine(&init, initArgv.data(), initArgv.size());

    WslcContainerSettings settings;
    WslcInitContainerSettings(image, &settings);
    WslcSetContainerSettingsInitProcess(&settings, &init);
    if (autoRemove)
        WslcSetContainerSettingsFlags(&settings, WSLC_CONTAINER_FLAG_AUTO_REMOVE);
    std::vector<DriveMount> drives = shareDrives ? Drives() : std::vector<DriveMount>();
    std::vector<WslcContainerVolume> volumes;
    for (auto& drive : drives) {
        WslcContainerVolume volume = {};
        volume.windowsPath = drive.windowsPath.c_str();
        volume.containerPath = drive.containerPath.c_str();
        volume.readOnly = FALSE;
        volumes.push_back(volume);
    }
    HRESULT hr;
    if (!volumes.empty()) {
        hr = WslcSetContainerSettingsVolumes(&settings, volumes.data(), (uint32_t)volumes.size());
        if (FAILED(hr)) {
            Fail(L"couldn't share the drives with the container", hr, nullptr);
            return hr;
        }
    }
    hr = WslcCreateContainer(session, &settings, container, &error);
    if (FAILED(hr)) {
        Fail(L"couldn't create the container", hr, error);
        return hr;
    }
    WslcProcessCallbacks callbacks = {};
    callbacks.onStdOut = OnInitOutput;
    callbacks.onStdErr = OnInitOutput;
    callbacks.onExit = OnInitExit;
    bool attached = SUCCEEDED(WslcSetContainerInitProcessIOCallbacks(*container, &callbacks, watch));
    hr = WslcStartContainer(*container, attached ? WSLC_CONTAINER_START_FLAG_ATTACH : WSLC_CONTAINER_START_FLAG_NONE, &error);
    if (FAILED(hr)) {
        Fail(L"couldn't start the container", hr, error);
        ExplainStopped(*container, watch);
        return hr;
    }
    // A container that can't keep running stops within moments.
    Sleep(1500);
    WslcContainerState state = WSLC_CONTAINER_STATE_INVALID;
    WslcGetContainerState(*container, &state);
    bool exited;
    {
        std::lock_guard<std::mutex> guard(watch->lock);
        exited = watch->exited;
    }
    if (state != WSLC_CONTAINER_STATE_RUNNING || exited) {
        fwprintf(stderr, L"[pfx-render] error the container stopped as soon as it started (state %d)\n", (int)state);
        ExplainStopped(*container, watch);
        return E_FAIL;
    }
    return S_OK;
}

static int Run(const char* image, const std::vector<std::string>& command)
{
    WslcSession session = nullptr;
    WslcContainer container = nullptr;
    WslcProcess process = nullptr;
    Exit exit = { CreateEventW(nullptr, TRUE, FALSE, nullptr), -1 };
    int result = EXIT_SETUP_FAILED;
    PWSTR error = nullptr;
    HRESULT hr;
    std::vector<PCSTR> argv;
    InitWatch watch;

    Status(L"starting the renderer");
    hr = OpenSession(&session);
    if (FAILED(hr))
        goto cleanup;
    hr = EnsureImage(session, image);
    if (FAILED(hr))
        goto cleanup;
    // The container's first process only keeps it running while the render runs.
    hr = StartContainer(session, image, true, true, { "/bin/sleep", "infinity" }, &container, &watch);
    if (FAILED(hr))
        goto cleanup;

    {
        // The image's entrypoint sets up MoonRay's environment, then runs the command.
        argv.push_back("/entrypoint.sh");
        for (auto& part : command)
            argv.push_back(part.c_str());
        WslcProcessSettings settings;
        WslcInitProcessSettings(&settings);
        WslcSetProcessSettingsCmdLine(&settings, argv.data(), argv.size());
        WslcProcessCallbacks callbacks = {};
        callbacks.onStdOut = OnOutput;
        callbacks.onStdErr = OnOutput;
        callbacks.onExit = OnExit;
        WslcSetProcessSettingsCallbacks(&settings, &callbacks, &exit);
        Status(L"rendering");
        hr = WslcCreateContainerProcess(container, &settings, &process, &error);
        if (FAILED(hr)) {
            Fail(L"couldn't run the renderer", hr, error);
            ExplainStopped(container, &watch);
            goto cleanup;
        }
        HANDLE waits[2] = { exit.event, g_cancelled };
        DWORD which = WaitForMultipleObjects(2, waits, FALSE, INFINITE);
        if (which == WAIT_OBJECT_0) {
            result = exit.code;
        } else {
            Status(L"cancelled");
            result = 130;
        }
    }

cleanup:
    if (process)
        WslcReleaseProcess(process);
    if (container) {
        WslcStopContainer(container, WSLC_SIGNAL_SIGTERM, 5, nullptr);
        WslcReleaseContainer(container);
    }
    if (session) {
        WslcTerminateSession(session);
        WslcReleaseSession(session);
    }
    if (exit.event)
        CloseHandle(exit.event);
    return result;
}

// ---------------------------------------------------------------------------
// Setup
// ---------------------------------------------------------------------------

static void Missing(WslcComponentFlags missing)
{
    if (missing & WSLC_COMPONENT_FLAG_VIRTUAL_MACHINE_PLATFORM)
        Status(L"missing: Virtual Machine Platform (a Windows feature; installing it needs a restart)");
    if (missing & WSLC_COMPONENT_FLAG_WSL_PACKAGE)
        Status(L"missing: an up-to-date WSL");
}

// Whether this program runs with administrator rights (installing WSL needs them).
static bool IsElevated()
{
    HANDLE token = nullptr;
    TOKEN_ELEVATION elevation = {};
    DWORD size = 0;
    bool elevated = false;
    if (OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &token)) {
        elevated = GetTokenInformation(token, TokenElevation, &elevation, sizeof(elevation), &size) && elevation.TokenIsElevated;
        CloseHandle(token);
    }
    return elevated;
}

// Installs as an administrator: runs a second copy of this program through Windows'
// "run as administrator" request (the usual approval prompt), which writes what it
// did to a log; that's passed on here. Its exit code is returned.
static int InstallElevated()
{
    wchar_t exe[MAX_PATH];
    GetModuleFileNameW(nullptr, exe, MAX_PATH);
    wchar_t temp[MAX_PATH];
    GetTempPathW(MAX_PATH, temp);
    std::wstring log = std::wstring(temp) + L"pfx-render-install-" + std::to_wstring(GetCurrentProcessId()) + L".log";
    std::wstring parameters = L"--install --log \"" + log + L"\"";
    Status(L"asking Windows for administrator approval");

    SHELLEXECUTEINFOW run = { sizeof(run) };
    run.fMask = SEE_MASK_NOCLOSEPROCESS;
    run.lpVerb = L"runas";
    run.lpFile = exe;
    run.lpParameters = parameters.c_str();
    run.nShow = SW_HIDE;
    if (!ShellExecuteExW(&run)) {
        DWORD error = GetLastError();
        if (error == ERROR_CANCELLED) {
            fwprintf(stderr, L"[pfx-render] error installing needs administrator approval, which wasn't given. An administrator can run this once instead: %s\n", ADMIN_COMMAND);
            fflush(stderr);
            return EXIT_NEEDS_ADMIN;
        }
        Fail(L"couldn't ask for administrator approval", HRESULT_FROM_WIN32(error), nullptr);
        return EXIT_SETUP_FAILED;
    }
    WaitForSingleObject(run.hProcess, INFINITE);
    DWORD code = EXIT_SETUP_FAILED;
    GetExitCodeProcess(run.hProcess, &code);
    CloseHandle(run.hProcess);

    // What the administrator copy reported.
    FILE* file = nullptr;
    if (_wfopen_s(&file, log.c_str(), L"r") == 0 && file) {
        char line[2048];
        while (fgets(line, sizeof(line), file))
            fputs(line, stderr);
        fclose(file);
        DeleteFileW(log.c_str());
    }
    fflush(stderr);
    return (int)code;
}

static void CALLBACK OnInstall(WslcComponentFlags component, uint32_t step, uint32_t steps, PVOID)
{
    fwprintf(stderr, L"[pfx-render] install %u %u %s\n", step, steps,
        component & WSLC_COMPONENT_FLAG_VIRTUAL_MACHINE_PLATFORM ? L"Virtual Machine Platform" : L"WSL");
    fflush(stderr);
}

int wmain(int argc, wchar_t* argv[])
{
    CoInitializeEx(nullptr, COINIT_MULTITHREADED);
    g_cancelled = CreateEventW(nullptr, TRUE, FALSE, nullptr);
    int result = 0;

    // The image: a newer one can be tried with PFX_MOONRAY_IMAGE.
    char imageBuffer[512] = PFX_MOONRAY_IMAGE;
    DWORD length = GetEnvironmentVariableA("PFX_MOONRAY_IMAGE", imageBuffer, sizeof(imageBuffer));
    if (length == 0 || length >= sizeof(imageBuffer))
        strcpy_s(imageBuffer, PFX_MOONRAY_IMAGE);
    const char* image = imageBuffer;

    std::wstring first = argc > 1 ? argv[1] : L"";
    WslcComponentFlags missing = WSLC_COMPONENT_FLAG_NONE;
    HRESULT ready = WslcGetMissingComponents(&missing);

    if (first == L"--check") {
        if (FAILED(ready)) {
            Fail(L"couldn't check for WSL containers", ready, nullptr);
            result = EXIT_NOT_READY;
        } else if (missing != WSLC_COMPONENT_FLAG_NONE) {
            Missing(missing);
            result = EXIT_NOT_READY;
        } else {
            WslcVersion version = {};
            WslcGetVersion(&version);
            fwprintf(stderr, L"[pfx-render] status ready (WSL containers %u.%u.%u; image %hs)\n", version.major, version.minor, version.revision, image);
        }
    } else if (first == L"--install") {
        // The administrator copy (see InstallElevated) writes its messages to a log.
        if (argc > 3 && std::wstring(argv[2]) == L"--log") {
            FILE* redirected = nullptr;
            _wfreopen_s(&redirected, argv[3], L"w", stderr);
        }
        if (SUCCEEDED(ready) && missing == WSLC_COMPONENT_FLAG_NONE) {
            Status(L"already installed");
        } else if (!IsElevated()) {
            Missing(missing);
            result = InstallElevated();
        } else {
            Missing(missing);
            HRESULT hr = WslcInstallWithDependencies(missing, WSLC_INSTALL_OPTION_NONE, OnInstall, nullptr);
            if (hr == HRESULT_FROM_WIN32(ERROR_CANCELLED) || hr == E_ACCESSDENIED || hr == HRESULT_FROM_WIN32(ERROR_ELEVATION_REQUIRED)) {
                // The administrator prompt was declined, or this account can't approve it.
                fwprintf(stderr, L"[pfx-render] error installing needs administrator approval, which wasn't given. An administrator can run this once instead: %s\n", ADMIN_COMMAND);
                fflush(stderr);
                result = EXIT_NEEDS_ADMIN;
            } else if (FAILED(hr)) {
                Fail(L"couldn't install WSL containers", hr, nullptr);
                result = EXIT_SETUP_FAILED;
            } else if (missing & WSLC_COMPONENT_FLAG_VIRTUAL_MACHINE_PLATFORM) {
                Status(L"installed; restart Windows to finish");
            } else {
                Status(L"installed");
            }
        }
    } else if (first == L"--diagnose") {
        // Tries containers a step at a time, to see which part stops them.
        WslcSession session = nullptr;
        result = EXIT_SETUP_FAILED;
        if (SUCCEEDED(OpenSession(&session)) && SUCCEEDED(EnsureImage(session, image))) {
            struct Try {
                const wchar_t* what;
                bool drives;
                std::vector<PCSTR> init;
            };
            std::vector<Try> tries = {
                { L"no drives shared, sleep infinity", false, { "/bin/sleep", "infinity" } },
                { L"no drives shared, sleep 3600", false, { "/bin/sleep", "3600" } },
                { L"drives shared, sleep infinity", true, { "/bin/sleep", "infinity" } },
            };
            int working = -1;
            for (size_t i = 0; i < tries.size(); i++) {
                fwprintf(stderr, L"[pfx-render] status try %zu: %s\n", i + 1, tries[i].what);
                WslcContainer container = nullptr;
                InitWatch watch;
                HRESULT hr = StartContainer(session, image, tries[i].drives, false, tries[i].init, &container, &watch);
                fwprintf(stderr, L"[pfx-render] status try %zu: %s\n", i + 1, SUCCEEDED(hr) ? L"keeps running" : L"stopped");
                if (SUCCEEDED(hr) && working < 0)
                    working = (int)i;
                if (container) {
                    WslcStopContainer(container, WSLC_SIGNAL_SIGTERM, 5, nullptr);
                    WslcReleaseContainer(container);
                }
            }
            if (working >= 0) {
                // MoonRay itself, in the first kind of container that stayed up.
                fwprintf(stderr, L"[pfx-render] status try 4: moonray -help in \"%s\"\n", tries[working].what);
                WslcContainer container = nullptr;
                InitWatch watch;
                if (SUCCEEDED(StartContainer(session, image, tries[working].drives, false, tries[working].init, &container, &watch))) {
                    WslcProcessSettings settings;
                    WslcInitProcessSettings(&settings);
                    PCSTR argv[] = { "/entrypoint.sh", "moonray", "-help" };
                    WslcSetProcessSettingsCmdLine(&settings, argv, 3);
                    Exit exit = { CreateEventW(nullptr, TRUE, FALSE, nullptr), -1 };
                    WslcProcessCallbacks callbacks = {};
                    callbacks.onStdOut = OnOutput;
                    callbacks.onStdErr = OnOutput;
                    callbacks.onExit = OnExit;
                    WslcSetProcessSettingsCallbacks(&settings, &callbacks, &exit);
                    WslcProcess process = nullptr;
                    PWSTR error = nullptr;
                    HRESULT hr = WslcCreateContainerProcess(container, &settings, &process, &error);
                    if (SUCCEEDED(hr)) {
                        WaitForSingleObject(exit.event, 60000);
                        fwprintf(stderr, L"[pfx-render] status try 4: moonray exited with %d\n", exit.code);
                        result = exit.code == 0 ? 0 : EXIT_SETUP_FAILED;
                        WslcReleaseProcess(process);
                    } else {
                        Fail(L"try 4: couldn't run moonray", hr, error);
                    }
                    CloseHandle(exit.event);
                }
                if (container) {
                    WslcStopContainer(container, WSLC_SIGNAL_SIGTERM, 5, nullptr);
                    WslcReleaseContainer(container);
                }
            }
        }
        if (session) {
            WslcTerminateSession(session);
            WslcReleaseSession(session);
        }
    } else if (first == L"--pull") {
        WslcSession session = nullptr;
        result = EXIT_SETUP_FAILED;
        if (SUCCEEDED(OpenSession(&session))) {
            if (SUCCEEDED(EnsureImage(session, image))) {
                Status(L"the MoonRay image is ready");
                result = 0;
            }
            WslcTerminateSession(session);
            WslcReleaseSession(session);
        }
    } else if (argc > 1) {
        if (FAILED(ready) || missing != WSLC_COMPONENT_FLAG_NONE) {
            Missing(missing);
            fwprintf(stderr, L"[pfx-render] error WSL containers aren't set up: run pfx-render.exe --install\n");
            result = EXIT_NOT_READY;
        } else {
            CreateThread(nullptr, 0, WatchInput, nullptr, 0, nullptr);
            std::vector<std::string> command;
            command.push_back(Utf8(argv[1])); // moonray, hd_render…
            for (int i = 2; i < argc; i++)
                command.push_back(MapArgument(argv[i]));
            result = Run(image, command);
        }
    } else {
        fwprintf(stderr, L"usage: pfx-render.exe --check | --install | --pull | --diagnose | moonray <args> | hd_render <args>\n");
        result = 2;
    }

    CoUninitialize();
    return result;
}
