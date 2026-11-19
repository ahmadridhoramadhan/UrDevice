// T16.1 probe (Windows): capture what the default speakers play, with WASAPI loopback.
// Build: cl /nologo /EHsc t16_1_wasapi_loopback.cpp ole32.lib
// Run:   t16_1_wasapi_loopback.exe [seconds]
// Prints the mix format and, per 500 ms, the number of frames received and the peak sample.
#include <windows.h>
#include <mmdeviceapi.h>
#include <audioclient.h>
#include <cstdio>
#include <cstdlib>
#include <cmath>

int main(int argc, char** argv) {
    int secs = argc > 1 ? atoi(argv[1]) : 6;
    CoInitializeEx(nullptr, COINIT_MULTITHREADED);
    IMMDeviceEnumerator* en = nullptr;
    HRESULT hr = CoCreateInstance(__uuidof(MMDeviceEnumerator), nullptr, CLSCTX_ALL, IID_PPV_ARGS(&en));
    wprintf(L"MMDeviceEnumerator hr=0x%08X\n", (unsigned)hr);
    IMMDevice* dev = nullptr;
    hr = en->GetDefaultAudioEndpoint(eRender, eConsole, &dev);
    wprintf(L"GetDefaultAudioEndpoint(eRender) hr=0x%08X\n", (unsigned)hr);
    if (FAILED(hr)) return 1;
    LPWSTR id = nullptr;
    dev->GetId(&id);
    wprintf(L"endpoint id: %s\n", id);
    CoTaskMemFree(id);
    IAudioClient* ac = nullptr;
    hr = dev->Activate(__uuidof(IAudioClient), CLSCTX_ALL, nullptr, (void**)&ac);
    wprintf(L"Activate(IAudioClient) hr=0x%08X\n", (unsigned)hr);
    WAVEFORMATEX* fmt = nullptr;
    ac->GetMixFormat(&fmt);
    wprintf(L"mix format: tag=0x%X channels=%u rate=%u bits=%u\n", fmt->wFormatTag, fmt->nChannels,
            (unsigned)fmt->nSamplesPerSec, fmt->wBitsPerSample);
    hr = ac->Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, 10000000, 0, fmt, nullptr);
    wprintf(L"Initialize(LOOPBACK) hr=0x%08X\n", (unsigned)hr);
    if (FAILED(hr)) return 1;
    IAudioCaptureClient* cap = nullptr;
    hr = ac->GetService(IID_PPV_ARGS(&cap));
    wprintf(L"GetService(IAudioCaptureClient) hr=0x%08X\n", (unsigned)hr);
    ac->Start();
    ULONGLONG t0 = GetTickCount64(), lastPrint = t0;
    UINT64 frames = 0, silentFrames = 0;
    float peak = 0;
    while (GetTickCount64() - t0 < (ULONGLONG)secs * 1000) {
        Sleep(10);
        UINT32 n = 0;
        while (SUCCEEDED(cap->GetNextPacketSize(&n)) && n > 0) {
            BYTE* data = nullptr;
            UINT32 got = 0;
            DWORD flags = 0;
            cap->GetBuffer(&data, &got, &flags, nullptr, nullptr);
            frames += got;
            if (flags & AUDCLNT_BUFFERFLAGS_SILENT) {
                silentFrames += got;
            } else if (fmt->wBitsPerSample == 32) {  // shared-mode mix format is float32
                const float* f = (const float*)data;
                for (UINT32 i = 0; i < got * fmt->nChannels; i++) peak = fmaxf(peak, fabsf(f[i]));
            }
            cap->ReleaseBuffer(got);
        }
        if (GetTickCount64() - lastPrint >= 500) {
            wprintf(L"t=%4llu ms frames=%llu (silent=%llu) peak=%.4f\n", GetTickCount64() - t0, frames, silentFrames, peak);
            peak = 0;
            lastPrint = GetTickCount64();
        }
    }
    ac->Stop();
    return 0;
}
