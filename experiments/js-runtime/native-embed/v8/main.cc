// Minimal V8 C++ embedding PoC for the FreeBSD 15 package build.
#include <iostream>
#include <memory>

#include <libplatform/libplatform.h>
#include <v8.h>

int main(int argc, char** argv)
{
    (void)argc;
    v8::V8::InitializeICUDefaultLocation(argv[0]);
    v8::V8::InitializeExternalStartupData(argv[0]);

    auto platform = v8::platform::NewDefaultPlatform();
    v8::V8::InitializePlatform(platform.get());
    if (!v8::V8::Initialize()) {
        std::cerr << "V8 initialization failed\n";
        return 1;
    }

    v8::Isolate::CreateParams params;
    params.array_buffer_allocator = v8::ArrayBuffer::Allocator::NewDefaultAllocator();
    v8::Isolate* isolate = v8::Isolate::New(params);
    if (!isolate) {
        std::cerr << "V8 isolate creation failed\n";
        delete params.array_buffer_allocator;
        v8::V8::Dispose();
        v8::V8::DisposePlatform();
        return 2;
    }

    int result_code = 0;
    {
        v8::Isolate::Scope isolate_scope(isolate);
        v8::HandleScope handle_scope(isolate);
        auto context = v8::Context::New(isolate);
        v8::Context::Scope context_scope(context);
        auto source = v8::String::NewFromUtf8(
            isolate, "1 + 2", v8::NewStringType::kNormal).ToLocalChecked();
        auto script = v8::Script::Compile(context, source).ToLocalChecked();
        auto result = script->Run(context).ToLocalChecked();
        v8::String::Utf8Value output(isolate, result);
        if (*output == nullptr) {
            std::cerr << "V8 result conversion failed\n";
            result_code = 3;
        } else {
            std::cout << *output << std::endl;
        }
    }

    isolate->Dispose();
    delete params.array_buffer_allocator;
    v8::V8::Dispose();
    v8::V8::DisposePlatform();
    return result_code;
}
