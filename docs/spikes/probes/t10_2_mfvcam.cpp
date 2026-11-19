// T10.2 probe (Windows 11): register a virtual camera with MFCreateVirtualCamera, then read frames
// back through Media Foundation to prove the camera works.
//
// The media source DLL is smourier/VCamSample's VCamSampleSource (MIT), built unmodified with cl.exe and
// registered with `regsvr32` (admin). Its CLSID is {3cad447d-f283-4af4-a3b2-6f5363309f52}.
//
// Build: cl /nologo /EHsc t10_2_mfvcam.cpp mf.lib mfplat.lib mfuuid.lib mfreadwrite.lib mfsensorgroup.lib ole32.lib
// Run:   t10_2_mfvcam.exe <CLSID with braces> [frames] [out.bmp]
#include <windows.h>
#include <mfapi.h>
#include <mfidl.h>
#include <mfreadwrite.h>
#include <mfvirtualcamera.h>
#include <cstdio>
#include <cstdlib>

static const wchar_t* kName = L"UrDevice Probe Camera";

static IMFActivate* find_device() {
    IMFAttributes* a = nullptr;
    MFCreateAttributes(&a, 1);
    a->SetGUID(MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID);
    IMFActivate** devs = nullptr;
    UINT32 n = 0;
    MFEnumDeviceSources(a, &devs, &n);
    IMFActivate* found = nullptr;
    for (UINT32 i = 0; i < n; i++) {
        WCHAR* name = nullptr;
        UINT32 len = 0;
        devs[i]->GetAllocatedString(MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME, &name, &len);
        wprintf(L"  device: %s\n", name ? name : L"?");
        if (name && wcsncmp(name, kName, wcslen(kName)) == 0 && !found) { found = devs[i]; found->AddRef(); }
        CoTaskMemFree(name);
        devs[i]->Release();
    }
    CoTaskMemFree(devs);
    a->Release();
    return found;
}

static void write_bmp(const char* path, int w, int h, const BYTE* bgra, int stride) {
    FILE* f = fopen(path, "wb");
    if (!f) return;
    BITMAPFILEHEADER fh = {0x4D42, (DWORD)(54 + w * h * 4), 0, 0, 54};
    BITMAPINFOHEADER ih = {40, w, -h, 1, 32, BI_RGB, (DWORD)(w * h * 4), 0, 0, 0, 0};
    fwrite(&fh, sizeof fh, 1, f);
    fwrite(&ih, sizeof ih, 1, f);
    for (int y = 0; y < h; y++) fwrite(bgra + (size_t)y * stride, 4, w, f);
    fclose(f);
}

int main(int argc, char** argv) {
    if (argc < 2) { wprintf(L"usage: t10_2_mfvcam {CLSID} [frames] [out.bmp]\n"); return 2; }
    wchar_t clsid[64];
    MultiByteToWideChar(CP_ACP, 0, argv[1], -1, clsid, 64);
    int want = argc > 2 ? atoi(argv[2]) : 30;
    const char* bmp = argc > 3 ? argv[3] : nullptr;

    CoInitializeEx(nullptr, COINIT_MULTITHREADED);
    HRESULT hr = MFStartup(MF_VERSION);
    wprintf(L"MFStartup hr=0x%08X\n", (unsigned)hr);

    IMFVirtualCamera* cam = nullptr;
    hr = MFCreateVirtualCamera(MFVirtualCameraType_SoftwareCameraSource, MFVirtualCameraLifetime_Session,
                               MFVirtualCameraAccess_CurrentUser, kName, clsid, nullptr, 0, &cam);
    wprintf(L"MFCreateVirtualCamera hr=0x%08X\n", (unsigned)hr);
    if (FAILED(hr)) return 1;
    hr = cam->Start(nullptr);
    wprintf(L"IMFVirtualCamera::Start hr=0x%08X\n", (unsigned)hr);
    if (SUCCEEDED(hr)) {
        Sleep(1500);
        IMFActivate* act = find_device();
        wprintf(L"virtual camera enumerated: %s\n", act ? L"yes" : L"no");
        if (act) {
            IMFMediaSource* src = nullptr;
            hr = act->ActivateObject(IID_PPV_ARGS(&src));
            wprintf(L"ActivateObject hr=0x%08X\n", (unsigned)hr);
            if (SUCCEEDED(hr)) {
                IMFAttributes* ra = nullptr;
                MFCreateAttributes(&ra, 1);
                ra->SetUINT32(MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, TRUE);
                IMFSourceReader* rd = nullptr;
                hr = MFCreateSourceReaderFromMediaSource(src, ra, &rd);
                wprintf(L"MFCreateSourceReaderFromMediaSource hr=0x%08X\n", (unsigned)hr);
                IMFMediaType* mt = nullptr;
                MFCreateMediaType(&mt);
                mt->SetGUID(MF_MT_MAJOR_TYPE, MFMediaType_Video);
                mt->SetGUID(MF_MT_SUBTYPE, MFVideoFormat_RGB32);
                hr = rd->SetCurrentMediaType((DWORD)MF_SOURCE_READER_FIRST_VIDEO_STREAM, nullptr, mt);
                wprintf(L"SetCurrentMediaType RGB32 hr=0x%08X\n", (unsigned)hr);
                IMFMediaType* cur = nullptr;
                UINT32 w = 0, h = 0, fn = 0, fd = 0;
                if (SUCCEEDED(rd->GetCurrentMediaType((DWORD)MF_SOURCE_READER_FIRST_VIDEO_STREAM, &cur))) {
                    MFGetAttributeSize(cur, MF_MT_FRAME_SIZE, &w, &h);
                    MFGetAttributeRatio(cur, MF_MT_FRAME_RATE, &fn, &fd);
                    wprintf(L"format: %ux%u @ %u/%u fps\n", w, h, fn, fd);
                }
                int got = 0;
                ULONGLONG t0 = GetTickCount64();
                while (got < want && GetTickCount64() - t0 < 15000) {
                    DWORD si = 0, fl = 0;
                    LONGLONG ts = 0;
                    IMFSample* s = nullptr;
                    hr = rd->ReadSample((DWORD)MF_SOURCE_READER_FIRST_VIDEO_STREAM, 0, &si, &fl, &ts, &s);
                    if (FAILED(hr)) { wprintf(L"ReadSample hr=0x%08X\n", (unsigned)hr); break; }
                    if (!s) continue;
                    got++;
                    if (got == 1 && bmp && w && h) {
                        IMFMediaBuffer* b = nullptr;
                        s->ConvertToContiguousBuffer(&b);
                        BYTE* p = nullptr;
                        DWORD ml = 0, cl = 0;
                        b->Lock(&p, &ml, &cl);
                        write_bmp(bmp, (int)w, (int)h, p, (int)w * 4);
                        b->Unlock();
                        b->Release();
                    }
                    s->Release();
                }
                wprintf(L"frames read: %d in %llu ms\n", got, GetTickCount64() - t0);
                src->Shutdown();
            }
            act->Release();
        }
    }
    cam->Remove();
    cam->Shutdown();
    cam->Release();
    MFShutdown();
    return 0;
}
