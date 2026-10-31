#include <cstring>
#include <memory>

#include <jsapi.h>
#include <js/CharacterEncoding.h>
#include <js/CompilationAndEvaluation.h>
#include <js/Context.h>
#include <js/Conversions.h>
#include <js/GlobalObject.h>
#include <js/HeapAPI.h>
#include <js/Initialization.h>
#include <js/Realm.h>
#include <js/RealmOptions.h>
#include <js/RootingAPI.h>
#include <js/SourceText.h>

static const JSClass global_class = {
    "aibe-global", JSCLASS_GLOBAL_FLAGS, &JS::DefaultGlobalClassOps,
    nullptr, nullptr, nullptr
};

struct AibeSpiderMonkeyRuntime {
    JSContext* context = nullptr;
    std::unique_ptr<JS::PersistentRootedObject> global;
};

extern "C" void* aibe_spidermonkey_create()
{
    if (!JS_Init()) return nullptr;
    auto runtime = std::make_unique<AibeSpiderMonkeyRuntime>();
    runtime->context = JS_NewContext(JS::DefaultHeapMaxBytes);
    if (runtime->context == nullptr) {
        JS_ShutDown();
        return nullptr;
    }
    if (!JS::InitSelfHostedCode(runtime->context)) {
        JS_DestroyContext(runtime->context);
        JS_ShutDown();
        return nullptr;
    }

    bool global_initialized = false;
    {
        JS::RealmOptions options;
        JS::RootedObject global(runtime->context, JS_NewGlobalObject(
            runtime->context, &global_class, nullptr, JS::FireOnNewGlobalHook, options));
        if (global) {
            {
                JSAutoRealm realm(runtime->context, global);
                global_initialized = JS::InitRealmStandardClasses(runtime->context);
            }
            if (global_initialized) {
                runtime->global = std::make_unique<JS::PersistentRootedObject>(runtime->context, global);
            }
        }
    }
    if (!global_initialized) {
        JS_DestroyContext(runtime->context);
        JS_ShutDown();
        return nullptr;
    }
    return runtime.release();
}

extern "C" int aibe_spidermonkey_eval(void* opaque, const char* source, char* output, size_t capacity)
{
    if (opaque == nullptr || source == nullptr || output == nullptr || capacity == 0) return -1;
    auto* runtime = static_cast<AibeSpiderMonkeyRuntime*>(opaque);
    JSContext* cx = runtime->context;
    JSAutoRealm realm(cx, *runtime->global);

    JS::CompileOptions options(cx);
    options.setFileAndLine("aibe-ffi-poc.js", 1);
    JS::SourceText<mozilla::Utf8Unit> text;
    if (!text.init(cx, source, std::strlen(source), JS::SourceOwnership::Borrowed)) return -2;

    JS::RootedValue value(cx);
    if (!JS::Evaluate(cx, options, text, &value)) {
        JS_ClearPendingException(cx);
        return -3;
    }
    JS::RootedString string(cx, JS::ToString(cx, value));
    if (!string) {
        JS_ClearPendingException(cx);
        return -4;
    }
    JS::UniqueChars utf8 = JS_EncodeStringToUTF8(cx, string);
    if (!utf8) return -5;
    const size_t length = std::strlen(utf8.get());
    if (length + 1 > capacity) return -6;
    std::memcpy(output, utf8.get(), length + 1);
    return 0;
}

extern "C" void aibe_spidermonkey_destroy(void* opaque)
{
    if (opaque == nullptr) return;
    auto* runtime = static_cast<AibeSpiderMonkeyRuntime*>(opaque);
    runtime->global.reset();
    JS_DestroyContext(runtime->context);
    JS_ShutDown();
    delete runtime;
}
