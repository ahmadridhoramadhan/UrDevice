// T10.5 probe (Windows): can a plain (unpackaged) exe use UserNotificationListener?
// Build: cl /nologo /EHsc /std:c++17 t10_5_unl.cpp windowsapp.lib
// Run:   t10_5_unl.exe
// Prints the package identity of the process, then tries RequestAccessAsync and GetNotificationsAsync.
#include <windows.h>
#include <appmodel.h>
#include <winrt/base.h>
#include <winrt/Windows.Foundation.h>
#include <winrt/Windows.Foundation.Collections.h>
#include <winrt/Windows.ApplicationModel.h>
#include <winrt/Windows.UI.Notifications.h>
#include <winrt/Windows.UI.Notifications.Management.h>
#include <cstdio>
#include <cstdlib>
#include <string>

using namespace winrt;
using namespace Windows::UI::Notifications;
using namespace Windows::UI::Notifications::Management;

int main(int argc, char** argv) {
    init_apartment();
    if (argc > 1 && std::string(argv[1]) == "events") {
        // Subscribe to NotificationChanged and count events for N seconds (default 20).
        int secs = argc > 2 ? atoi(argv[2]) : 20;
        try {
            auto listener = UserNotificationListener::Current();
            wprintf(L"access=%d\n", (int)listener.RequestAccessAsync().get());
            volatile long events = 0;
            auto token = listener.NotificationChanged([&](auto&&, UserNotificationChangedEventArgs const& a) {
                InterlockedIncrement(&events);
                wprintf(L"  NotificationChanged kind=%d id=%u\n", (int)a.ChangeKind(), a.UserNotificationId());
            });
            wprintf(L"subscribed; waiting %d s\n", secs);
            Sleep(secs * 1000);
            listener.NotificationChanged(token);
            wprintf(L"events seen: %ld\n", events);
        } catch (hresult_error const& e) {
            wprintf(L"EXCEPTION hr=0x%08X %s\n", (unsigned)e.code().value, e.message().c_str());
        }
        return 0;
    }
    UINT32 len = 0;
    LONG rc = GetCurrentPackageFullName(&len, nullptr);
    wprintf(L"GetCurrentPackageFullName rc=%ld (15700 = APPMODEL_ERROR_NO_PACKAGE: no package identity)\n", rc);
    try {
        auto listener = UserNotificationListener::Current();
        wprintf(L"UserNotificationListener::Current ok\n");
        auto before = listener.GetAccessStatus();
        wprintf(L"GetAccessStatus = %d (0 Unspecified, 1 Allowed, 2 Denied)\n", (int)before);
        auto status = listener.RequestAccessAsync().get();
        wprintf(L"RequestAccessAsync = %d\n", (int)status);
        auto list = listener.GetNotificationsAsync(NotificationKinds::Toast).get();
        wprintf(L"GetNotificationsAsync ok, count=%u\n", list.Size());
        // Privacy: print only the app id, the notification id, and the lengths of the text fields.
        int shown = 0;
        for (auto const& n : list) {
            if (shown++ >= 3) break;
            wprintf(L"  id=%u\n", n.Id());
            try { n.CreationTime(); wprintf(L"    CreationTime ok\n"); } catch (hresult_error const& e) { wprintf(L"    CreationTime hr=0x%08X\n", (unsigned)e.code().value); }
            try { auto a = n.AppInfo(); wprintf(L"    AppInfo ok\n"); try { auto u = a.AppUserModelId(); wprintf(L"    AppUserModelId=%s\n", u.c_str()); } catch (hresult_error const& e) { wprintf(L"    AppUserModelId hr=0x%08X\n", (unsigned)e.code().value); } try { auto d = a.DisplayInfo(); wprintf(L"    DisplayInfo ok\n"); auto nm = d.DisplayName(); wprintf(L"    DisplayName=%s\n", nm.c_str()); } catch (hresult_error const& e) { wprintf(L"    DisplayInfo hr=0x%08X\n", (unsigned)e.code().value); } } catch (hresult_error const& e) { wprintf(L"    AppInfo hr=0x%08X\n", (unsigned)e.code().value); }
            try {
                auto binding = n.Notification().Visual().GetBinding(KnownNotificationBindings::ToastGeneric());
                size_t chars = 0;
                if (binding) for (auto const& t : binding.GetTextElements()) chars += t.Text().size();
                wprintf(L"    Visual binding ok, text_chars=%zu\n", chars);
            } catch (hresult_error const& e) { wprintf(L"    Visual hr=0x%08X\n", (unsigned)e.code().value); }
        }
    } catch (hresult_error const& e) {
        wprintf(L"EXCEPTION hr=0x%08X %s\n", (unsigned)e.code().value, e.message().c_str());
    }
    return 0;
}
