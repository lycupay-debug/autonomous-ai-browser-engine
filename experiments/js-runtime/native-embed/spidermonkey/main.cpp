// Minimal SpiderMonkey JSAPI embedding PoC for the FreeBSD 15 package build.
#include <cstdio>
#include <cstring>

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

int main()
{
    if (!JS_Init()) {
        std::fprintf(stderr, "JS_Init failed\n");
        return 1;
    }

    JSContext* cx = JS_NewContext(JS::DefaultHeapMaxBytes);
    if (!cx) {
        std::fprintf(stderr, "JS_NewContext failed\n");
        JS_ShutDown();
        return 2;
    }

    if (!JS::InitSelfHostedCode(cx)) {
        std::fprintf(stderr, "JS::InitSelfHostedCode failed\n");
        JS_DestroyContext(cx);
        JS_ShutDown();
        return 3;
    }

    int result_code = 0;
    {
        JS::RealmOptions realm_options;
        JS::RootedObject global(cx, JS_NewGlobalObject(
            cx, &global_class, nullptr, JS::FireOnNewGlobalHook, realm_options));
        if (!global) {
            std::fprintf(stderr, "JS_NewGlobalObject failed\n");
            result_code = 4;
        } else {
            JSAutoRealm realm(cx, global);
            if (!JS::InitRealmStandardClasses(cx)) {
                std::fprintf(stderr, "InitRealmStandardClasses failed\n");
                result_code = 5;
            } else {
                const char* code = "1 + 2";
                JS::CompileOptions options(cx);
                options.setFileAndLine("aibe-sm-embed.js", 1);
                JS::SourceText<mozilla::Utf8Unit> source;
                if (!source.init(cx, code, std::strlen(code), JS::SourceOwnership::Borrowed)) {
                    std::fprintf(stderr, "SourceText initialization failed\n");
                    result_code = 6;
                } else {
                    JS::RootedValue value(cx);
                    if (!JS::Evaluate(cx, options, source, &value)) {
                        std::fprintf(stderr, "JS::Evaluate failed\n");
                        result_code = 7;
                    } else {
                        JS::RootedString string(cx, JS::ToString(cx, value));
                        JS::UniqueChars utf8 = JS_EncodeStringToUTF8(cx, string);
                        if (!utf8) {
                            std::fprintf(stderr, "JS_EncodeStringToUTF8 failed\n");
                            result_code = 8;
                        } else {
                            std::printf("%s\n", utf8.get());
                        }
                    }
                }
            }
        }
    }

    JS_DestroyContext(cx);
    JS_ShutDown();
    return result_code;
}
