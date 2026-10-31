#include <algorithm>
#include <cstdlib>
#include <cstring>
#include <memory>

#include <libplatform/libplatform.h>
#include <v8.h>

struct AibeV8Runtime {
    std::unique_ptr<v8::Platform> platform;
    v8::ArrayBuffer::Allocator* allocator = nullptr;
    v8::Isolate* isolate = nullptr;
    v8::Global<v8::Context> context;
};

extern "C" void* aibe_v8_create()
{
    const char* executable = std::getenv("AIBE_V8_EXECUTABLE");
    if (executable == nullptr) executable = "/usr/local/bin/d8";
    v8::V8::InitializeICUDefaultLocation(executable);
    v8::V8::InitializeExternalStartupData(executable);

    auto runtime = std::make_unique<AibeV8Runtime>();
    runtime->platform = v8::platform::NewDefaultPlatform();
    v8::V8::InitializePlatform(runtime->platform.get());
    if (!v8::V8::Initialize()) {
        v8::V8::DisposePlatform();
        return nullptr;
    }

    runtime->allocator = v8::ArrayBuffer::Allocator::NewDefaultAllocator();
    v8::Isolate::CreateParams params;
    params.array_buffer_allocator = runtime->allocator;
    runtime->isolate = v8::Isolate::New(params);
    if (runtime->isolate == nullptr) {
        delete runtime->allocator;
        v8::V8::Dispose();
        v8::V8::DisposePlatform();
        return nullptr;
    }

    {
        v8::Isolate::Scope isolate_scope(runtime->isolate);
        v8::HandleScope handle_scope(runtime->isolate);
        auto context = v8::Context::New(runtime->isolate);
        runtime->context.Reset(runtime->isolate, context);
    }

    return runtime.release();
}

extern "C" int aibe_v8_eval(void* opaque, const char* source, char* output, size_t capacity)
{
    if (opaque == nullptr || source == nullptr || output == nullptr || capacity == 0) return -1;
    auto* runtime = static_cast<AibeV8Runtime*>(opaque);
    v8::Isolate::Scope isolate_scope(runtime->isolate);
    v8::HandleScope handle_scope(runtime->isolate);
    auto context = runtime->context.Get(runtime->isolate);
    v8::Context::Scope context_scope(context);
    v8::TryCatch try_catch(runtime->isolate);

    v8::Local<v8::String> text;
    if (!v8::String::NewFromUtf8(runtime->isolate, source, v8::NewStringType::kNormal).ToLocal(&text)) return -2;
    v8::Local<v8::Script> script;
    if (!v8::Script::Compile(context, text).ToLocal(&script)) return -3;
    v8::Local<v8::Value> value;
    if (!script->Run(context).ToLocal(&value)) return -4;

    v8::String::Utf8Value utf8(runtime->isolate, value);
    if (*utf8 == nullptr) return -5;
    const size_t length = static_cast<size_t>(utf8.length());
    if (length + 1 > capacity) return -6;
    std::memcpy(output, *utf8, length);
    output[length] = '\0';
    return 0;
}

extern "C" void aibe_v8_destroy(void* opaque)
{
    if (opaque == nullptr) return;
    auto* runtime = static_cast<AibeV8Runtime*>(opaque);
    runtime->context.Reset();
    runtime->isolate->Dispose();
    delete runtime->allocator;
    v8::V8::Dispose();
    v8::V8::DisposePlatform();
    delete runtime;
}
