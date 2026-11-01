// FreeBSD V8 embedding boundary. V8 platform initialization is process-wide;
// each AIBE runtime owns one isolate and one context on its creating thread.
#include <algorithm>
#include <atomic>
#include <cstdlib>
#include <cstdint>
#include <cstring>
#include <memory>
#include <mutex>
#include <thread>
#include <string>
#include <utility>
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

struct AibeEventListener {
    uint64_t id;
    std::string type;
    v8::Global<v8::Function> callback;
};

struct AibeNativeNode {
    std::vector<std::shared_ptr<AibeNativeNode>> children;
    std::weak_ptr<AibeNativeNode> parent;
    std::vector<AibeEventListener> listeners;
    uint64_t next_listener_id = 1;

    // Persistent V8 callback handles must be released before the isolate dies.
    ~AibeNativeNode()
    {
        for (auto& listener : listeners) {
            listener.callback.Reset();
        }
    }
};

struct AibeNodeWrapper {
    AibeV8Runtime* owner;
    std::shared_ptr<AibeNativeNode> node;
    v8::Global<v8::Object> object;
    AibeNodeWrapper* pending_next = nullptr;
};

struct AibeV8Runtime {
    std::unique_ptr<v8::ArrayBuffer::Allocator> allocator;
    v8::Isolate* isolate = nullptr;
    v8::Global<v8::Context> context;
    std::thread::id owner_thread;
    v8::Global<v8::FunctionTemplate> node_template;
    // Weak per-runtime identity cache: it must not keep JS wrappers alive.
    std::unordered_map<AibeNativeNode*, AibeNodeWrapper*> node_wrappers;
    // Weak callbacks only enqueue wrappers; native/V8 handle destruction is
    // deferred until the isolate has left the GC callback phase.
    AibeNodeWrapper* pending_node_releases = nullptr;
};

namespace {
std::atomic<size_t> g_finalized_dom_wrappers{0};

void drain_pending_node_releases(AibeV8Runtime* runtime)
{
    auto* wrapper = runtime->pending_node_releases;
    runtime->pending_node_releases = nullptr;
    while (wrapper != nullptr) {
        auto* next = wrapper->pending_next;
        delete wrapper;
        wrapper = next;
    }
}

struct PendingNodeReleaseScope {
    AibeV8Runtime* runtime;
    ~PendingNodeReleaseScope() { drain_pending_node_releases(runtime); }
};

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
    auto* owner = wrapper->owner;
    owner->node_wrappers.erase(wrapper->node.get());
    wrapper->pending_next = owner->pending_node_releases;
    owner->pending_node_releases = wrapper;
    g_finalized_dom_wrappers.fetch_add(1, std::memory_order_relaxed);
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

void node_add_event_listener(const v8::FunctionCallbackInfo<v8::Value>& info)
{
    auto* isolate = info.GetIsolate();
    auto* wrapper = node_wrapper_from(info.This());
    if (wrapper == nullptr || info.Length() < 2 || !info[0]->IsString()
        || !info[1]->IsFunction()) {
        isolate->ThrowException(v8::Exception::TypeError(
            v8::String::NewFromUtf8Literal(isolate, "addEventListener requires a type and callback")));
        return;
    }

    v8::String::Utf8Value event_type(isolate, info[0]);
    if (*event_type == nullptr || event_type.length() > 256) {
        isolate->ThrowException(v8::Exception::TypeError(
            v8::String::NewFromUtf8Literal(isolate, "event type must not exceed 256 UTF-8 bytes")));
        return;
    }
    const std::string type(*event_type, static_cast<size_t>(event_type.length()));
    auto callback = info[1].As<v8::Function>();
    for (const auto& listener : wrapper->node->listeners) {
        if (listener.type == type && listener.callback.Get(isolate)->StrictEquals(callback)) {
            return;
        }
    }

    if (wrapper->node->listeners.size() >= 1024) {
        isolate->ThrowException(v8::Exception::RangeError(
            v8::String::NewFromUtf8Literal(isolate, "Node listener limit exceeded")));
        return;
    }

    if (wrapper->node->next_listener_id == 0) {
        isolate->ThrowException(v8::Exception::RangeError(
            v8::String::NewFromUtf8Literal(isolate, "Node listener identifier space exhausted")));
        return;
    }

    AibeEventListener listener;
    listener.id = wrapper->node->next_listener_id++;
    listener.type = type;
    listener.callback.Reset(isolate, callback);
    wrapper->node->listeners.push_back(std::move(listener));
}

void node_remove_event_listener(const v8::FunctionCallbackInfo<v8::Value>& info)
{
    auto* isolate = info.GetIsolate();
    auto* wrapper = node_wrapper_from(info.This());
    if (wrapper == nullptr || info.Length() < 2 || !info[0]->IsString()
        || !info[1]->IsFunction()) {
        isolate->ThrowException(v8::Exception::TypeError(
            v8::String::NewFromUtf8Literal(isolate, "removeEventListener requires a type and callback")));
        return;
    }

    v8::String::Utf8Value event_type(isolate, info[0]);
    if (*event_type == nullptr) {
        return;
    }
    if (event_type.length() > 256) {
        isolate->ThrowException(v8::Exception::TypeError(
            v8::String::NewFromUtf8Literal(isolate, "event type exceeds 256 UTF-8 bytes")));
        return;
    }
    const std::string type(*event_type, static_cast<size_t>(event_type.length()));
    auto callback = info[1].As<v8::Function>();
    auto& listeners = wrapper->node->listeners;
    listeners.erase(
        std::remove_if(listeners.begin(), listeners.end(), [&](AibeEventListener& listener) {
            if (listener.type != type || !listener.callback.Get(isolate)->StrictEquals(callback)) {
                return false;
            }
            listener.callback.Reset();
            return true;
        }),
        listeners.end());
}

void node_dispatch_event(const v8::FunctionCallbackInfo<v8::Value>& info)
{
    auto* isolate = info.GetIsolate();
    auto* wrapper = node_wrapper_from(info.This());
    if (wrapper == nullptr || info.Length() < 1 || !info[0]->IsString()) {
        isolate->ThrowException(v8::Exception::TypeError(
            v8::String::NewFromUtf8Literal(isolate, "dispatchEvent requires an event type string")));
        return;
    }

    v8::String::Utf8Value event_type(isolate, info[0]);
    if (*event_type == nullptr) {
        return;
    }
    const std::string type(*event_type, static_cast<size_t>(event_type.length()));

    // Snapshot callbacks before invoking script: listeners may add or remove
    // listeners re-entrantly, invalidating the native vector during dispatch.
    std::vector<std::pair<uint64_t, v8::Local<v8::Function>>> callbacks;
    for (const auto& listener : wrapper->node->listeners) {
        if (listener.type == type) {
            callbacks.emplace_back(listener.id, listener.callback.Get(isolate));
        }
    }

    auto context = isolate->GetCurrentContext();
    auto event_value = v8::String::NewFromUtf8(
        isolate, type.c_str(), v8::NewStringType::kNormal,
        static_cast<int>(type.size())).ToLocalChecked();
    v8::Local<v8::Value> arguments[] = {event_value};
    int invoked = 0;
    for (const auto& [listener_id, callback] : callbacks) {
        // A listener removed by an earlier callback in this dispatch must not
        // run later in the same dispatch; newly added listeners are not in the snapshot.
        const bool still_registered = std::any_of(
            wrapper->node->listeners.begin(),
            wrapper->node->listeners.end(),
            [listener_id](const AibeEventListener& listener) {
                return listener.id == listener_id;
            });
        if (!still_registered) {
            continue;
        }
        if (callback->Call(context, info.This(), 1, arguments).IsEmpty()) {
            return;
        }
        ++invoked;
    }
    info.GetReturnValue().Set(v8::Integer::New(isolate, invoked));
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
        "addEventListener",
        v8::FunctionTemplate::New(isolate, node_add_event_listener));
    node_template->PrototypeTemplate()->Set(
        isolate,
        "removeEventListener",
        v8::FunctionTemplate::New(isolate, node_remove_event_listener));
    node_template->PrototypeTemplate()->Set(
        isolate,
        "dispatchEvent",
        v8::FunctionTemplate::New(isolate, node_dispatch_event));
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
    PendingNodeReleaseScope pending_release_scope{runtime};
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
    PendingNodeReleaseScope pending_release_scope{runtime};
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
    PendingNodeReleaseScope pending_release_scope{runtime};
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
    {
        v8::Isolate::Scope isolate_scope(runtime->isolate);
        drain_pending_node_releases(runtime);
        runtime->context.Reset();
        runtime->node_template.Reset();
        for (const auto& entry : runtime->node_wrappers) {
            auto* wrapper = entry.second;
            wrapper->object.Reset();
            delete wrapper;
        }
        runtime->node_wrappers.clear();
    }
    runtime->isolate->Dispose();
    runtime->isolate = nullptr;
    delete runtime;
    // V8 and its platform are intentionally process-lifetime singletons. Calling
    // V8::Dispose per isolate would invalidate other live AIBE runtime instances.
    return 0;
}
