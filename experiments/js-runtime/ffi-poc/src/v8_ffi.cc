// FreeBSD V8 embedding boundary. V8 platform initialization is process-wide;
// each AIBE runtime owns one isolate and one context on its creating thread.
#include <algorithm>
#include <atomic>
#include <cstdlib>
#include <cstring>
#include <memory>
#include <mutex>
#include <thread>
#include <unordered_map>
#include <vector>

#include <libplatform/libplatform.h>
#include <v8.h>

namespace {
std::once_flag g_v8_initialization_once;
bool g_v8_initialized = false;
v8::Platform* g_v8_platform = nullptr; // Process-lifetime; V8 owns worker threads.

bool initialize_v8_once(const char* executable)
{
    std::call_once(g_v8_initialization_once, [executable] {
        v8::V8::InitializeICUDefaultLocation(executable);
        v8::V8::InitializeExternalStartupData(executable);
        auto platform = v8::platform::NewDefaultPlatform();
        if (!platform) {
            return;
        }
        g_v8_platform = platform.release();
        v8::V8::InitializePlatform(g_v8_platform);
        g_v8_initialized = v8::V8::Initialize();
    });
    return g_v8_initialized;
}
} // namespace

struct AibeV8Runtime;

struct AibeNativeNode {
    std::vector<std::shared_ptr<AibeNativeNode>> children;
    std::weak_ptr<AibeNativeNode> parent;
};

struct AibeNodeWrapper {
    AibeV8Runtime* owner;
    std::shared_ptr<AibeNativeNode> node;
    v8::Global<v8::Object> object;
};

struct AibeV8Runtime {
    std::unique_ptr<v8::ArrayBuffer::Allocator> allocator;
    v8::Isolate* isolate = nullptr;
    v8::Global<v8::Context> context;
    std::thread::id owner_thread;
    v8::Global<v8::FunctionTemplate> node_template;
    // Weak per-runtime identity cache: it must not keep JS wrappers alive.
    std::unordered_map<AibeNativeNode*, AibeNodeWrapper*> node_wrappers;
};

namespace {
std::atomic<size_t> g_finalized_dom_wrappers{0};

AibeNodeWrapper* node_wrapper_from(v8::Local<v8::Object> object)
{
    if (object.IsEmpty() || object->InternalFieldCount() != 1) {
        return nullptr;
    }
    auto external = object->GetInternalField(0);
    if (external.IsEmpty() || !external.As<v8::Value>()->IsExternal()) {
        return nullptr;
    }
    return static_cast<AibeNodeWrapper*>(
        external.As<v8::External>()->Value(v8::kExternalPointerTypeTagDefault));
}

void node_wrapper_weak_callback(const v8::WeakCallbackInfo<AibeNodeWrapper>& info)
{
    auto* wrapper = info.GetParameter();
    wrapper->object.Reset();
    wrapper->owner->node_wrappers.erase(wrapper->node.get());
    g_finalized_dom_wrappers.fetch_add(1, std::memory_order_relaxed);
    delete wrapper;
}

// Attach one JS object to a native Node and register its weak identity entry.
// The weak callback removes the cache entry when JS no longer retains the object.
AibeNodeWrapper* attach_node_wrapper(
    AibeV8Runtime* runtime,
    v8::Local<v8::Object> object,
    const std::shared_ptr<AibeNativeNode>& node)
{
    auto* isolate = runtime->isolate;
    auto* wrapper = new AibeNodeWrapper{runtime, node, {}};
    object->SetInternalField(
        0, v8::External::New(isolate, wrapper, v8::kExternalPointerTypeTagDefault));
    wrapper->object.Reset(isolate, object);
    wrapper->object.SetWeak(wrapper, node_wrapper_weak_callback, v8::WeakCallbackType::kParameter);
    runtime->node_wrappers.emplace(wrapper->node.get(), wrapper);
    return wrapper;
}

// Return the existing wrapper for this native identity, or create a new one.
// Native ownership may outlive a JS wrapper, so re-wrapping is an expected path.
v8::MaybeLocal<v8::Object> wrap_node(
    AibeV8Runtime* runtime,
    const std::shared_ptr<AibeNativeNode>& node)
{
    auto* isolate = runtime->isolate;
    auto found = runtime->node_wrappers.find(node.get());
    if (found != runtime->node_wrappers.end()) {
        auto object = found->second->object.Get(isolate);
        if (!object.IsEmpty()) {
            return object;
        }
        return {};
    }

    auto node_template = runtime->node_template.Get(isolate);
    if (node_template.IsEmpty()) {
        return {};
    }
    v8::Local<v8::Object> object;
    if (!node_template->InstanceTemplate()
             ->NewInstance(isolate->GetCurrentContext())
             .ToLocal(&object)) {
        return {};
    }
    attach_node_wrapper(runtime, object, node);
    return object;
}

void node_constructor(const v8::FunctionCallbackInfo<v8::Value>& info)
{
    auto* isolate = info.GetIsolate();
    auto* runtime = static_cast<AibeV8Runtime*>(isolate->GetData(0));
    if (runtime == nullptr || info.This()->InternalFieldCount() != 1) {
        isolate->ThrowException(v8::Exception::Error(
            v8::String::NewFromUtf8Literal(isolate, "Node constructor is unavailable")));
        return;
    }

    attach_node_wrapper(runtime, info.This(), std::make_shared<AibeNativeNode>());
    info.GetReturnValue().Set(info.This());
}

void node_append_child(const v8::FunctionCallbackInfo<v8::Value>& info)
{
    auto* isolate = info.GetIsolate();
    auto* parent = node_wrapper_from(info.This());
    if (parent == nullptr || info.Length() < 1 || !info[0]->IsObject()) {
        isolate->ThrowException(v8::Exception::TypeError(
            v8::String::NewFromUtf8Literal(isolate, "appendChild requires a Node")));
        return;
    }
    auto* child = node_wrapper_from(info[0].As<v8::Object>());
    if (child == nullptr || child->owner != parent->owner) {
        isolate->ThrowException(v8::Exception::TypeError(
            v8::String::NewFromUtf8Literal(isolate, "appendChild received an invalid Node")));
        return;
    }

    // DOM insertion must not create an ancestor cycle. The native ownership
    // graph uses strong parent-to-child edges and weak child-to-parent edges;
    // without this check, a cycle would leak native nodes after JS GC.
    for (auto ancestor = parent->node; ancestor; ancestor = ancestor->parent.lock()) {
        if (ancestor == child->node) {
            isolate->ThrowException(v8::Exception::Error(
                v8::String::NewFromUtf8Literal(isolate, "HierarchyRequestError")));
            return;
        }
    }

    // Moving a child detaches it from the previous parent's strong child list.
    if (auto previous_parent = child->node->parent.lock()) {
        auto& siblings = previous_parent->children;
        siblings.erase(
            std::remove(siblings.begin(), siblings.end(), child->node),
            siblings.end());
    }
    child->node->parent = parent->node;
    parent->node->children.push_back(child->node);
    info.GetReturnValue().Set(info[0]);
}

void node_child_count(const v8::FunctionCallbackInfo<v8::Value>& info)
{
    auto* isolate = info.GetIsolate();
    auto* wrapper = node_wrapper_from(info.This());
    if (wrapper == nullptr) {
        isolate->ThrowException(v8::Exception::TypeError(
            v8::String::NewFromUtf8Literal(isolate, "childCount receiver is not a Node")));
        return;
    }
    info.GetReturnValue().Set(v8::Integer::New(
        isolate, static_cast<int>(wrapper->node->children.size())));
}

// Child access must pass through wrap_node so repeated reads preserve === identity.
void node_first_child(const v8::FunctionCallbackInfo<v8::Value>& info)
{
    auto* isolate = info.GetIsolate();
    auto* parent = node_wrapper_from(info.This());
    if (parent == nullptr) {
        isolate->ThrowException(v8::Exception::TypeError(
            v8::String::NewFromUtf8Literal(isolate, "firstChild receiver is not a Node")));
        return;
    }
    if (parent->node->children.empty()) {
        info.GetReturnValue().Set(v8::Null(isolate));
        return;
    }

    v8::Local<v8::Object> child;
    if (!wrap_node(parent->owner, parent->node->children.front()).ToLocal(&child)) {
        isolate->ThrowException(v8::Exception::Error(
            v8::String::NewFromUtf8Literal(isolate, "unable to create Node wrapper")));
        return;
    }
    info.GetReturnValue().Set(child);
}

bool install_node_class(AibeV8Runtime* runtime, v8::Local<v8::Context> context)
{
    auto* isolate = runtime->isolate;
    auto node_template = v8::FunctionTemplate::New(isolate, node_constructor);
    runtime->node_template.Reset(isolate, node_template);
    node_template->SetClassName(v8::String::NewFromUtf8Literal(isolate, "Node"));
    node_template->InstanceTemplate()->SetInternalFieldCount(1);
    node_template->PrototypeTemplate()->Set(
        isolate,
        "appendChild",
        v8::FunctionTemplate::New(isolate, node_append_child));
    node_template->PrototypeTemplate()->Set(
        isolate,
        "childCount",
        v8::FunctionTemplate::New(isolate, node_child_count));
    node_template->PrototypeTemplate()->Set(
        isolate,
        "firstChild",
        v8::FunctionTemplate::New(isolate, node_first_child));
    v8::Local<v8::Function> constructor;
    if (!node_template->GetFunction(context).ToLocal(&constructor)) {
        return false;
    }
    return context->Global()
        ->Set(context, v8::String::NewFromUtf8Literal(isolate, "Node"), constructor)
        .FromMaybe(false);
}
} // namespace

extern "C" void* aibe_v8_create()
{
    const char* executable = std::getenv("AIBE_V8_EXECUTABLE");
    if (executable == nullptr) {
        executable = "/usr/local/bin/d8";
    }
    if (!initialize_v8_once(executable)) {
        return nullptr;
    }

    auto runtime = std::make_unique<AibeV8Runtime>();
    runtime->owner_thread = std::this_thread::get_id();
    runtime->allocator.reset(v8::ArrayBuffer::Allocator::NewDefaultAllocator());
    if (!runtime->allocator) {
        return nullptr;
    }

    v8::Isolate::CreateParams params;
    params.array_buffer_allocator = runtime->allocator.get();
    runtime->isolate = v8::Isolate::New(params);
    if (runtime->isolate == nullptr) {
        return nullptr;
    }
    runtime->isolate->SetMicrotasksPolicy(v8::MicrotasksPolicy::kExplicit);

    {
        v8::Isolate::Scope isolate_scope(runtime->isolate);
        v8::HandleScope handle_scope(runtime->isolate);
        auto context = v8::Context::New(runtime->isolate);
        if (context.IsEmpty()) {
            runtime->isolate->Dispose();
            runtime->isolate = nullptr;
            return nullptr;
        }
        runtime->isolate->SetData(0, runtime.get());
        if (!install_node_class(runtime.get(), context)) {
            runtime->isolate->Dispose();
            runtime->isolate = nullptr;
            return nullptr;
        }
        runtime->context.Reset(runtime->isolate, context);
    }

    return runtime.release();
}

extern "C" int aibe_v8_eval(
    void* opaque,
    const char* source,
    char* output,
    size_t capacity,
    size_t* output_length)
{
    if (opaque == nullptr || source == nullptr || output == nullptr || output_length == nullptr) {
        return -1;
    }
    *output_length = 0;
    auto* runtime = static_cast<AibeV8Runtime*>(opaque);
    if (std::this_thread::get_id() != runtime->owner_thread) {
        return -7;
    }

    v8::Isolate::Scope isolate_scope(runtime->isolate);
    v8::HandleScope handle_scope(runtime->isolate);
    auto context = runtime->context.Get(runtime->isolate);
    if (context.IsEmpty()) {
        return -8;
    }
    v8::Context::Scope context_scope(context);
    v8::TryCatch try_catch(runtime->isolate);

    v8::Local<v8::String> text;
    if (!v8::String::NewFromUtf8(
             runtime->isolate, source, v8::NewStringType::kNormal).ToLocal(&text)) {
        return -2;
    }
    v8::Local<v8::Script> script;
    if (!v8::Script::Compile(context, text).ToLocal(&script)) {
        return -3;
    }
    v8::Local<v8::Value> value;
    if (!script->Run(context).ToLocal(&value)) {
        return -4;
    }

    v8::String::Utf8Value utf8(runtime->isolate, value);
    if (*utf8 == nullptr || utf8.length() < 0) {
        return -5;
    }
    const size_t length = static_cast<size_t>(utf8.length());
    if (length > capacity) {
        return -6;
    }
    std::memcpy(output, *utf8, length);
    *output_length = length;
    return 0;
}

extern "C" int aibe_v8_run_microtasks(void* opaque)
{
    if (opaque == nullptr) {
        return -1;
    }
    auto* runtime = static_cast<AibeV8Runtime*>(opaque);
    if (std::this_thread::get_id() != runtime->owner_thread) {
        return -7;
    }
    v8::Isolate::Scope isolate_scope(runtime->isolate);
    runtime->isolate->PerformMicrotaskCheckpoint();
    return 0;
}

extern "C" int aibe_v8_force_gc(void* opaque)
{
    if (opaque == nullptr) {
        return -1;
    }
    auto* runtime = static_cast<AibeV8Runtime*>(opaque);
    if (std::this_thread::get_id() != runtime->owner_thread) {
        return -7;
    }
    v8::Isolate::Scope isolate_scope(runtime->isolate);
    runtime->isolate->LowMemoryNotification();
    return 0;
}

extern "C" size_t aibe_v8_finalized_dom_wrapper_count()
{
    return g_finalized_dom_wrappers.load(std::memory_order_relaxed);
}

extern "C" int aibe_v8_destroy(void* opaque)
{
    if (opaque == nullptr) {
        return 0;
    }
    auto* runtime = static_cast<AibeV8Runtime*>(opaque);
    if (std::this_thread::get_id() != runtime->owner_thread) {
        return -7;
    }
    runtime->context.Reset();
    runtime->node_template.Reset();
    for (const auto& entry : runtime->node_wrappers) {
        auto* wrapper = entry.second;
        wrapper->object.Reset();
        delete wrapper;
    }
    runtime->node_wrappers.clear();
    runtime->isolate->Dispose();
    runtime->isolate = nullptr;
    delete runtime;
    // V8 and its platform are intentionally process-lifetime singletons. Calling
    // V8::Dispose per isolate would invalidate other live AIBE runtime instances.
    return 0;
}
