// FreeBSD V8 embedding boundary. V8 platform initialization is process-wide;
// each AIBE runtime owns one isolate and one context on its creating thread.
#include <algorithm>
#include <array>
#include <atomic>
#include <cstdlib>
#include <cstdint>
#include <cstring>
#include <functional>
#include <memory>
#include <mutex>
#include <thread>
#include <string>
#include <utility>
#include <unordered_map>
#include <vector>

#include <cppgc/allocation.h>
#include <cppgc/member.h>
#include <libplatform/libplatform.h>
#include <v8-cppgc.h>
#include <v8-sandbox.h>
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
    uint64_t realm_id;
    v8::Global<v8::Object> object;
    AibeNodeWrapper* pending_next = nullptr;
};

struct AibeNodeWrapperKey {
    AibeNativeNode* node;
    uint64_t realm_id;

    bool operator==(const AibeNodeWrapperKey& other) const noexcept
    {
        return node == other.node && realm_id == other.realm_id;
    }
};

struct AibeNodeWrapperKeyHash {
    size_t operator()(const AibeNodeWrapperKey& key) const noexcept
    {
        const auto node_hash = std::hash<AibeNativeNode*>{}(key.node);
        const auto realm_hash = std::hash<uint64_t>{}(key.realm_id);
        return node_hash ^ (realm_hash + 0x9e3779b9U + (node_hash << 6U) + (node_hash >> 2U));
    }
};

using AibeHostCallback = int (*)(
    void*, const uint8_t*, size_t, uint8_t*, size_t, size_t*);

struct AibeV8Runtime {
    std::unique_ptr<v8::ArrayBuffer::Allocator> allocator;
    v8::Isolate* isolate = nullptr;
    v8::Global<v8::Context> context;
    // Additional isolated V8 Contexts are addressed by opaque monotonic IDs.
    std::unordered_map<uint64_t, v8::Global<v8::Context>> realms;
    uint64_t next_realm_id = 1;
    std::thread::id owner_thread;
    AibeHostCallback host_callback = nullptr;
    void* host_callback_data = nullptr;
    uint64_t host_callback_realm_id = 0;
    v8::Global<v8::FunctionTemplate> node_template;
    // Weak per-runtime, per-Realm identity cache; it never roots JS wrappers.
    // Native identity is scoped by Realm: the same host node may have one
    // wrapper per Realm, but wrappers are never silently reused across Worlds.
    std::unordered_map<AibeNodeWrapperKey, AibeNodeWrapper*, AibeNodeWrapperKeyHash> node_wrappers;
    // Weak callbacks only enqueue wrappers; native/V8 handle destruction is
    // deferred until the isolate has left the GC callback phase.
    AibeNodeWrapper* pending_node_releases = nullptr;
};

namespace {
constexpr int kAibeRealmEmbedderDataIndex = 1;
constexpr uint64_t kInvalidRealmId = UINT64_MAX;

uint64_t realm_id_from_context(v8::Local<v8::Context> context)
{
    if (context.IsEmpty()) {
        return kInvalidRealmId;
    }
    auto data = context->GetEmbedderData(kAibeRealmEmbedderDataIndex);
    if (data.IsEmpty() || !data->IsBigInt()) {
        return kInvalidRealmId;
    }
    bool lossless = false;
    const auto realm_id = data.As<v8::BigInt>()->Uint64Value(&lossless);
    return lossless ? realm_id : kInvalidRealmId;
}

uint64_t current_realm_id(v8::Isolate* isolate)
{
    return realm_id_from_context(isolate->GetCurrentContext());
}

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

AibeNodeWrapper* node_wrapper_from(
    v8::Isolate* isolate,
    v8::Local<v8::Object> object)
{
    if (object.IsEmpty() || object->InternalFieldCount() != 1) {
        return nullptr;
    }
    auto external = object->GetInternalField(0);
    if (external.IsEmpty() || !external.As<v8::Value>()->IsExternal()) {
        return nullptr;
    }
    auto* wrapper = static_cast<AibeNodeWrapper*>(
        external.As<v8::External>()->Value(v8::kExternalPointerTypeTagDefault));
    if (wrapper == nullptr || wrapper->owner->isolate != isolate
        || wrapper->realm_id != current_realm_id(isolate)) {
        return nullptr;
    }
    return wrapper;
}

void node_wrapper_weak_callback(const v8::WeakCallbackInfo<AibeNodeWrapper>& info)
{
    auto* wrapper = info.GetParameter();
    wrapper->object.Reset();
    auto* owner = wrapper->owner;
    owner->node_wrappers.erase(AibeNodeWrapperKey{wrapper->node.get(), wrapper->realm_id});
    wrapper->pending_next = owner->pending_node_releases;
    owner->pending_node_releases = wrapper;
    g_finalized_dom_wrappers.fetch_add(1, std::memory_order_relaxed);
}

// Attach one JS object to a native Node and register its weak identity entry.
// The weak callback removes the cache entry when JS no longer retains the object.
AibeNodeWrapper* attach_node_wrapper(
    AibeV8Runtime* runtime,
    v8::Local<v8::Object> object,
    const std::shared_ptr<AibeNativeNode>& node,
    uint64_t realm_id)
{
    auto* isolate = runtime->isolate;
    auto* wrapper = new AibeNodeWrapper{runtime, node, realm_id, {}};
    object->SetInternalField(
        0, v8::External::New(isolate, wrapper, v8::kExternalPointerTypeTagDefault));
    wrapper->object.Reset(isolate, object);
    wrapper->object.SetWeak(wrapper, node_wrapper_weak_callback, v8::WeakCallbackType::kParameter);
    runtime->node_wrappers.emplace(AibeNodeWrapperKey{wrapper->node.get(), realm_id}, wrapper);
    return wrapper;
}

// Return the existing wrapper for this native identity, or create a new one.
// Native ownership may outlive a JS wrapper, so re-wrapping is an expected path.
v8::MaybeLocal<v8::Object> wrap_node(
    AibeV8Runtime* runtime,
    const std::shared_ptr<AibeNativeNode>& node,
    uint64_t realm_id)
{
    auto* isolate = runtime->isolate;
    if (current_realm_id(isolate) != realm_id) {
        return {};
    }
    const AibeNodeWrapperKey key{node.get(), realm_id};
    auto found = runtime->node_wrappers.find(key);
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
    attach_node_wrapper(runtime, object, node, realm_id);
    return object;
}

void node_constructor(const v8::FunctionCallbackInfo<v8::Value>& info)
{
    auto* isolate = info.GetIsolate();
    auto* runtime = static_cast<AibeV8Runtime*>(isolate->GetData(0));
    const auto realm_id = current_realm_id(isolate);
    if (runtime == nullptr || realm_id == kInvalidRealmId
        || info.This()->InternalFieldCount() != 1) {
        isolate->ThrowException(v8::Exception::Error(
            v8::String::NewFromUtf8Literal(isolate, "Node constructor is unavailable")));
        return;
    }

    attach_node_wrapper(runtime, info.This(), std::make_shared<AibeNativeNode>(), realm_id);
    info.GetReturnValue().Set(info.This());
}

void node_append_child(const v8::FunctionCallbackInfo<v8::Value>& info)
{
    auto* isolate = info.GetIsolate();
    auto* parent = node_wrapper_from(isolate, info.This());
    if (parent == nullptr || info.Length() < 1 || !info[0]->IsObject()) {
        isolate->ThrowException(v8::Exception::TypeError(
            v8::String::NewFromUtf8Literal(isolate, "appendChild requires a Node")));
        return;
    }
    auto* child = node_wrapper_from(isolate, info[0].As<v8::Object>());
    if (child == nullptr || child->owner != parent->owner
        || child->realm_id != parent->realm_id) {
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

void node_remove_child(const v8::FunctionCallbackInfo<v8::Value>& info)
{
    auto* isolate = info.GetIsolate();
    auto* parent = node_wrapper_from(isolate, info.This());
    if (parent == nullptr || info.Length() < 1 || !info[0]->IsObject()) {
        isolate->ThrowException(v8::Exception::TypeError(
            v8::String::NewFromUtf8Literal(isolate, "removeChild requires a Node")));
        return;
    }
    auto* child = node_wrapper_from(isolate, info[0].As<v8::Object>());
    if (child == nullptr || child->owner != parent->owner
        || child->realm_id != parent->realm_id) {
        isolate->ThrowException(v8::Exception::TypeError(
            v8::String::NewFromUtf8Literal(isolate, "removeChild received an invalid Node")));
        return;
    }

    auto& children = parent->node->children;
    auto found = std::find(children.begin(), children.end(), child->node);
    if (found == children.end()) {
        isolate->ThrowException(v8::Exception::Error(
            v8::String::NewFromUtf8Literal(isolate, "NotFoundError")));
        return;
    }
    child->node->parent.reset();
    children.erase(found);
    info.GetReturnValue().Set(info[0]);
}

constexpr size_t kAibeHostCallbackMaxBytes = 16 * 1024;

void host_bridge_callback(const v8::FunctionCallbackInfo<v8::Value>& info)
{
    auto* isolate = info.GetIsolate();
    auto* runtime = static_cast<AibeV8Runtime*>(isolate->GetData(0));
    const auto current_context = isolate->GetCurrentContext();
    const auto incumbent_context = isolate->GetIncumbentContext();
    if (runtime == nullptr || runtime->host_callback == nullptr
        || current_context.IsEmpty() || incumbent_context.IsEmpty()
        || realm_id_from_context(current_context) != runtime->host_callback_realm_id
        || realm_id_from_context(incumbent_context) != runtime->host_callback_realm_id) {
        isolate->ThrowException(v8::Exception::Error(
            v8::String::NewFromUtf8Literal(isolate, "host callback capability is unavailable")));
        return;
    }
    if (info.Length() != 1 || !info[0]->IsString()) {
        isolate->ThrowException(v8::Exception::TypeError(
            v8::String::NewFromUtf8Literal(isolate, "hostCall requires exactly one string argument")));
        return;
    }

    v8::String::Utf8Value input(isolate, info[0]);
    if (*input == nullptr || input.length() < 0) {
        isolate->ThrowException(v8::Exception::TypeError(
            v8::String::NewFromUtf8Literal(isolate, "hostCall input is not valid UTF-8")));
        return;
    }
    const size_t input_length = static_cast<size_t>(input.length());
    if (input_length > kAibeHostCallbackMaxBytes) {
        isolate->ThrowException(v8::Exception::RangeError(
            v8::String::NewFromUtf8Literal(isolate, "hostCall input exceeds the 16 KiB limit")));
        return;
    }

    std::array<uint8_t, kAibeHostCallbackMaxBytes> output{};
    size_t output_length = 0;
    const int status = runtime->host_callback(
        runtime->host_callback_data,
        reinterpret_cast<const uint8_t*>(*input), input_length,
        output.data(), output.size(), &output_length);
    if (output_length > output.size()) {
        isolate->ThrowException(v8::Exception::Error(
            v8::String::NewFromUtf8Literal(isolate, "host callback returned an invalid length")));
        return;
    }

    v8::Local<v8::String> result;
    if (!v8::String::NewFromUtf8(
            isolate, reinterpret_cast<const char*>(output.data()),
            v8::NewStringType::kNormal, static_cast<int>(output_length)).ToLocal(&result)) {
        isolate->ThrowException(v8::Exception::Error(
            v8::String::NewFromUtf8Literal(isolate, "host callback result could not be decoded")));
        return;
    }
    if (status != 0) {
        isolate->ThrowException(v8::Exception::Error(result));
        return;
    }
    info.GetReturnValue().Set(result);
}

void node_add_event_listener(const v8::FunctionCallbackInfo<v8::Value>& info)
{
    auto* isolate = info.GetIsolate();
    auto* wrapper = node_wrapper_from(isolate, info.This());
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
    auto* wrapper = node_wrapper_from(isolate, info.This());
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
    auto* wrapper = node_wrapper_from(isolate, info.This());
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
    auto* wrapper = node_wrapper_from(isolate, info.This());
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
    auto* parent = node_wrapper_from(isolate, info.This());
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
    if (!wrap_node(
            parent->owner, parent->node->children.front(), parent->realm_id).ToLocal(&child)) {
        isolate->ThrowException(v8::Exception::Error(
            v8::String::NewFromUtf8Literal(isolate, "unable to create Node wrapper")));
        return;
    }
    info.GetReturnValue().Set(child);
}

bool initialize_node_template(AibeV8Runtime* runtime)
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
        "removeChild",
        v8::FunctionTemplate::New(isolate, node_remove_child));
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
    return true;
}

bool install_node_class(
    AibeV8Runtime* runtime,
    v8::Local<v8::Context> context,
    uint64_t realm_id)
{
    auto* isolate = runtime->isolate;
    context->SetEmbedderData(
        kAibeRealmEmbedderDataIndex, v8::BigInt::NewFromUnsigned(isolate, realm_id));
    auto node_template = runtime->node_template.Get(isolate);
    if (node_template.IsEmpty()) {
        return false;
    }
    v8::Local<v8::Function> constructor;
    if (!node_template->GetFunction(context).ToLocal(&constructor)) {
        return false;
    }
    return context->Global()
        ->Set(context, v8::String::NewFromUtf8Literal(isolate, "Node"), constructor)
        .FromMaybe(false);
}
std::atomic<size_t> g_finalized_cppgc_nodes{0};
constexpr auto kAibeCppGcNodeTag = v8::CppHeapPointerTag::kFirstEmbedderWrappableTag;

struct AibeCppGcListener {
    std::string type;
    v8::TracedReference<v8::Function> callback;
};

// Separate feasibility prototype for V8's unified V8/CppHeap tracing.
// This intentionally does not replace the existing shared_ptr DOM PoC.
class AibeCppGcNode final : public v8::Object::Wrappable {
public:
    cppgc::Member<AibeCppGcNode> child;
    std::vector<AibeCppGcListener> listeners;

    ~AibeCppGcNode() override
    {
        g_finalized_cppgc_nodes.fetch_add(1, std::memory_order_relaxed);
    }

    void Trace(cppgc::Visitor* visitor) const override
    {
        v8::Object::Wrappable::Trace(visitor);
        visitor->Trace(child);
        for (const auto& listener : listeners) {
            visitor->Trace(listener.callback);
        }
    }
};

AibeCppGcNode* cppgc_node_from(
    v8::Isolate* isolate,
    v8::Local<v8::Object> object)
{
    return v8::Object::Unwrap<kAibeCppGcNodeTag, AibeCppGcNode>(isolate, object);
}

void cppgc_node_constructor(const v8::FunctionCallbackInfo<v8::Value>& info)
{
    auto* isolate = info.GetIsolate();
    auto* heap = isolate->GetCppHeap();
    if (heap == nullptr) {
        isolate->ThrowException(v8::Exception::Error(
            v8::String::NewFromUtf8Literal(isolate, "CppHeap is not attached")));
        return;
    }
    auto* node = cppgc::MakeGarbageCollected<AibeCppGcNode>(heap->GetAllocationHandle());
    v8::Object::Wrap<kAibeCppGcNodeTag>(isolate, info.This(), node);
    info.GetReturnValue().Set(info.This());
}

void cppgc_node_add_event_listener(
    const v8::FunctionCallbackInfo<v8::Value>& info)
{
    auto* isolate = info.GetIsolate();
    auto* node = cppgc_node_from(isolate, info.This());
    if (node == nullptr || info.Length() < 2 || !info[0]->IsString()
        || !info[1]->IsFunction()) {
        isolate->ThrowException(v8::Exception::TypeError(
            v8::String::NewFromUtf8Literal(isolate, "addEventListener requires a type and callback")));
        return;
    }
    v8::String::Utf8Value type(isolate, info[0]);
    if (*type == nullptr || type.length() > 256) {
        isolate->ThrowException(v8::Exception::TypeError(
            v8::String::NewFromUtf8Literal(isolate, "event type exceeds 256 UTF-8 bytes")));
        return;
    }
    if (node->listeners.size() >= 1024) {
        isolate->ThrowException(v8::Exception::RangeError(
            v8::String::NewFromUtf8Literal(isolate, "Node listener limit exceeded")));
        return;
    }
    AibeCppGcListener listener;
    listener.type.assign(*type, static_cast<size_t>(type.length()));
    listener.callback.Reset(isolate, info[1].As<v8::Function>());
    node->listeners.push_back(std::move(listener));
}

void cppgc_node_append_child(const v8::FunctionCallbackInfo<v8::Value>& info)
{
    auto* isolate = info.GetIsolate();
    auto* parent = cppgc_node_from(isolate, info.This());
    if (parent == nullptr || info.Length() < 1 || !info[0]->IsObject()) {
        isolate->ThrowException(v8::Exception::TypeError(
            v8::String::NewFromUtf8Literal(isolate, "appendChild requires a Node")));
        return;
    }
    auto* child = cppgc_node_from(isolate, info[0].As<v8::Object>());
    if (child == nullptr || child == parent) {
        isolate->ThrowException(v8::Exception::TypeError(
            v8::String::NewFromUtf8Literal(isolate, "appendChild received an invalid Node")));
        return;
    }
    parent->child = child;
    info.GetReturnValue().Set(info[0]);
}

void cppgc_node_child_count(const v8::FunctionCallbackInfo<v8::Value>& info)
{
    auto* isolate = info.GetIsolate();
    auto* node = cppgc_node_from(isolate, info.This());
    if (node == nullptr) {
        isolate->ThrowException(v8::Exception::TypeError(
            v8::String::NewFromUtf8Literal(isolate, "childCount receiver is not a Node")));
        return;
    }
    info.GetReturnValue().Set(v8::Integer::New(isolate, node->child == nullptr ? 0 : 1));
}

bool install_cppgc_node_class(
    v8::Isolate* isolate,
    v8::Local<v8::Context> context)
{
    auto node_template = v8::FunctionTemplate::New(isolate, cppgc_node_constructor);
    node_template->SetClassName(v8::String::NewFromUtf8Literal(isolate, "CppGcNode"));
    node_template->PrototypeTemplate()->Set(
        isolate, "addEventListener",
        v8::FunctionTemplate::New(isolate, cppgc_node_add_event_listener));
    node_template->PrototypeTemplate()->Set(
        isolate, "appendChild",
        v8::FunctionTemplate::New(isolate, cppgc_node_append_child));
    node_template->PrototypeTemplate()->Set(
        isolate, "childCount",
        v8::FunctionTemplate::New(isolate, cppgc_node_child_count));
    v8::Local<v8::Function> constructor;
    if (!node_template->GetFunction(context).ToLocal(&constructor)) {
        return false;
    }
    return context->Global()
        ->Set(context, v8::String::NewFromUtf8Literal(isolate, "CppGcNode"), constructor)
        .FromMaybe(false);
}

bool run_cppgc_cycle_script(v8::Isolate* isolate)
{
    v8::Isolate::Scope isolate_scope(isolate);
    v8::HandleScope handle_scope(isolate);
    auto context = v8::Context::New(isolate);
    if (context.IsEmpty()) {
        return false;
    }
    v8::Context::Scope context_scope(context);
    if (!install_cppgc_node_class(isolate, context)) {
        return false;
    }

    auto source = v8::String::NewFromUtf8Literal(
        isolate,
        "globalThis.cycleRoot = new CppGcNode(); "
        "(() => { const root = cycleRoot; const child = new CppGcNode(); "
        "root.appendChild(child); "
        "child.addEventListener('cycle', () => child.childCount()); })(); "
        "cycleRoot = null;");
    v8::Local<v8::Script> script;
    if (!v8::Script::Compile(context, source).ToLocal(&script)) {
        return false;
    }
    v8::Local<v8::Value> result;
    return script->Run(context).ToLocal(&result);
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
        if (!initialize_node_template(runtime.get())
            || !install_node_class(runtime.get(), context, 0)) {
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

enum AibeJsValueTag : int {
    AIBE_JS_VALUE_TAG_UNDEFINED = 0,
    AIBE_JS_VALUE_TAG_NULL = 1,
    AIBE_JS_VALUE_TAG_BOOLEAN = 2,
    AIBE_JS_VALUE_TAG_NUMBER = 3,
    AIBE_JS_VALUE_TAG_STRING = 4,
    AIBE_JS_VALUE_TAG_UNSUPPORTED = 5,
};

static void copy_v8_exception_details(
    v8::Isolate* isolate,
    v8::TryCatch& try_catch,
    v8::Local<v8::Context> context,
    char* output,
    size_t capacity,
    size_t* output_length,
    uint32_t* error_line,
    uint32_t* error_column)
{
    if (output_length != nullptr) {
        *output_length = 0;
    }
    if (error_line != nullptr) {
        *error_line = 0;
    }
    if (error_column != nullptr) {
        *error_column = UINT32_MAX;
    }

    auto message = try_catch.Message();
    if (message.IsEmpty()) {
        return;
    }

    if (error_line != nullptr) {
        auto line = message->GetLineNumber(context);
        if (line.IsJust() && line.FromJust() > 0) {
            *error_line = static_cast<uint32_t>(line.FromJust());
        }
    }
    if (error_column != nullptr) {
        auto column = message->GetStartColumn(context);
        if (column.IsJust() && column.FromJust() >= 0) {
            *error_column = static_cast<uint32_t>(column.FromJust());
        }
    }

    if (output == nullptr || output_length == nullptr || capacity == 0) {
        return;
    }
    v8::String::Utf8Value utf8(isolate, message->Get());
    if (*utf8 == nullptr || utf8.length() < 0) {
        return;
    }
    const size_t length = std::min(static_cast<size_t>(utf8.length()), capacity);
    std::memcpy(output, *utf8, length);
    *output_length = length;
}

static int aibe_v8_eval_value_in_context(
    AibeV8Runtime* runtime,
    v8::Local<v8::Context> context,
    const char* source,
    int* value_tag,
    int* boolean_value,
    double* number_value,
    char* output,
    size_t capacity,
    size_t* output_length,
    uint32_t* error_line,
    uint32_t* error_column)
{
    if (runtime == nullptr || source == nullptr || value_tag == nullptr
        || boolean_value == nullptr || number_value == nullptr
        || output == nullptr || output_length == nullptr
        || error_line == nullptr || error_column == nullptr) {
        return -1;
    }
    *value_tag = -1;
    *boolean_value = 0;
    *number_value = 0.0;
    *output_length = 0;
    *error_line = 0;
    *error_column = UINT32_MAX;
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
        copy_v8_exception_details(
            runtime->isolate, try_catch, context, output, capacity,
            output_length, error_line, error_column);
        return -3;
    }
    v8::Local<v8::Value> value;
    if (!script->Run(context).ToLocal(&value)) {
        copy_v8_exception_details(
            runtime->isolate, try_catch, context, output, capacity,
            output_length, error_line, error_column);
        return -4;
    }

    if (value->IsUndefined()) {
        *value_tag = AIBE_JS_VALUE_TAG_UNDEFINED;
    } else if (value->IsNull()) {
        *value_tag = AIBE_JS_VALUE_TAG_NULL;
    } else if (value->IsBoolean()) {
        *value_tag = AIBE_JS_VALUE_TAG_BOOLEAN;
        *boolean_value = value->BooleanValue(runtime->isolate) ? 1 : 0;
    } else if (value->IsNumber()) {
        *value_tag = AIBE_JS_VALUE_TAG_NUMBER;
        *number_value = value.As<v8::Number>()->Value();
    } else if (value->IsString()) {
        *value_tag = AIBE_JS_VALUE_TAG_STRING;
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
    } else {
        // Objects, functions, symbols, and BigInts require explicit project
        // handles or dedicated representations; never coerce them implicitly.
        *value_tag = AIBE_JS_VALUE_TAG_UNSUPPORTED;
    }
    return 0;
}

static int aibe_v8_eval_value_for_realm(
    void* opaque,
    uint64_t realm_id,
    const char* source,
    int* value_tag,
    int* boolean_value,
    double* number_value,
    char* output,
    size_t capacity,
    size_t* output_length,
    uint32_t* error_line,
    uint32_t* error_column)
{
    if (opaque == nullptr) {
        return -1;
    }
    auto* runtime = static_cast<AibeV8Runtime*>(opaque);
    if (std::this_thread::get_id() != runtime->owner_thread) {
        return -7;
    }
    v8::Isolate::Scope isolate_scope(runtime->isolate);
    v8::HandleScope handle_scope(runtime->isolate);

    auto found = runtime->realms.find(realm_id);
    if (found == runtime->realms.end()) {
        return -9;
    }
    return aibe_v8_eval_value_in_context(
        runtime, found->second.Get(runtime->isolate), source,
        value_tag, boolean_value, number_value, output, capacity, output_length,
        error_line, error_column);
}

extern "C" int aibe_v8_eval_value(
    void* opaque,
    const char* source,
    int* value_tag,
    int* boolean_value,
    double* number_value,
    char* output,
    size_t capacity,
    size_t* output_length,
    uint32_t* error_line,
    uint32_t* error_column)
{
    if (opaque == nullptr) {
        return -1;
    }
    auto* runtime = static_cast<AibeV8Runtime*>(opaque);
    if (std::this_thread::get_id() != runtime->owner_thread) {
        return -7;
    }
    v8::Isolate::Scope isolate_scope(runtime->isolate);
    v8::HandleScope handle_scope(runtime->isolate);
    return aibe_v8_eval_value_in_context(
        runtime, runtime->context.Get(runtime->isolate), source,
        value_tag, boolean_value, number_value, output, capacity, output_length,
        error_line, error_column);
}

extern "C" int aibe_v8_create_realm(void* opaque, uint64_t* realm_id)
{
    if (opaque == nullptr || realm_id == nullptr) {
        return -1;
    }
    auto* runtime = static_cast<AibeV8Runtime*>(opaque);
    if (std::this_thread::get_id() != runtime->owner_thread) {
        return -7;
    }
    v8::Isolate::Scope isolate_scope(runtime->isolate);
    v8::HandleScope handle_scope(runtime->isolate);
    auto context = v8::Context::New(runtime->isolate);
    if (context.IsEmpty()) {
        return -8;
    }
    if (runtime->next_realm_id == 0 || runtime->next_realm_id == UINT64_MAX) {
        return -10;
    }
    const uint64_t id = runtime->next_realm_id++;
    if (!install_node_class(runtime, context, id)) {
        return -8;
    }
    v8::Global<v8::Context> persistent(runtime->isolate, context);
    runtime->realms.emplace(id, std::move(persistent));
    *realm_id = id;
    return 0;
}

extern "C" int aibe_v8_destroy_realm(void* opaque, uint64_t realm_id)
{
    if (opaque == nullptr || realm_id == 0) {
        return -1;
    }
    auto* runtime = static_cast<AibeV8Runtime*>(opaque);
    if (std::this_thread::get_id() != runtime->owner_thread) {
        return -7;
    }
    v8::Isolate::Scope isolate_scope(runtime->isolate);
    auto found = runtime->realms.find(realm_id);
    if (found == runtime->realms.end()) {
        return -9;
    }
    if (runtime->host_callback_realm_id == realm_id) {
        runtime->host_callback = nullptr;
        runtime->host_callback_data = nullptr;
        runtime->host_callback_realm_id = 0;
    }
    found->second.Reset();
    runtime->realms.erase(found);
    return 0;
}

extern "C" int aibe_v8_install_host_callback(
    void* opaque, uint64_t realm_id, AibeHostCallback callback, void* user_data)
{
    if (opaque == nullptr || realm_id == 0 || callback == nullptr || user_data == nullptr) {
        return -1;
    }
    auto* runtime = static_cast<AibeV8Runtime*>(opaque);
    if (std::this_thread::get_id() != runtime->owner_thread) {
        return -7;
    }
    if (runtime->host_callback != nullptr) {
        return -11;
    }
    auto found = runtime->realms.find(realm_id);
    if (found == runtime->realms.end()) {
        return -9;
    }

    v8::Isolate::Scope isolate_scope(runtime->isolate);
    v8::HandleScope handle_scope(runtime->isolate);
    auto context = found->second.Get(runtime->isolate);
    if (context.IsEmpty()) {
        return -8;
    }
    v8::Context::Scope context_scope(context);
    auto function = v8::Function::New(context, host_bridge_callback);
    v8::Local<v8::Function> host_function;
    if (!function.ToLocal(&host_function)
        || !context->Global()
                ->DefineOwnProperty(
                    context, v8::String::NewFromUtf8Literal(runtime->isolate, "hostCall"),
                    host_function, v8::None)
                .FromMaybe(false)) {
        return -8;
    }

    runtime->host_callback = callback;
    runtime->host_callback_data = user_data;
    runtime->host_callback_realm_id = realm_id;
    return 0;
}

// Test-only probe: transfer a granted host function into another Realm and
// verify that the callback checks the caller's active Realm, not just origin.
extern "C" int aibe_v8_host_callback_cross_realm_probe(
    void* opaque, uint64_t grant_realm_id, uint64_t caller_realm_id)
{
    if (opaque == nullptr || grant_realm_id == 0 || caller_realm_id == 0
        || grant_realm_id == caller_realm_id) {
        return -1;
    }
    auto* runtime = static_cast<AibeV8Runtime*>(opaque);
    if (std::this_thread::get_id() != runtime->owner_thread) {
        return -7;
    }
    auto grant = runtime->realms.find(grant_realm_id);
    auto caller = runtime->realms.find(caller_realm_id);
    if (grant == runtime->realms.end() || caller == runtime->realms.end()) {
        return -9;
    }

    v8::Isolate::Scope isolate_scope(runtime->isolate);
    v8::HandleScope handle_scope(runtime->isolate);
    v8::Local<v8::Function> host_function;
    {
        auto context = grant->second.Get(runtime->isolate);
        v8::Context::Scope context_scope(context);
        v8::Local<v8::Value> value;
        if (!context->Global()
                 ->Get(context, v8::String::NewFromUtf8Literal(runtime->isolate, "hostCall"))
                 .ToLocal(&value)
            || !value->IsFunction()) {
            return -8;
        }
        host_function = value.As<v8::Function>();
    }
    {
        auto context = caller->second.Get(runtime->isolate);
        v8::Context::Scope context_scope(context);
        if (!context->Global()
                 ->DefineOwnProperty(
                     context, v8::String::NewFromUtf8Literal(runtime->isolate, "hostCall"),
                     host_function, v8::None)
                 .FromMaybe(false)) {
            return -10;
        }
        auto source = v8::String::NewFromUtf8Literal(
            runtime->isolate,
            "try { hostCall('probe'); } catch (error) { error.message; }");
        v8::Local<v8::Script> script;
        if (!v8::Script::Compile(context, source).ToLocal(&script)) {
            return -11;
        }
        v8::Local<v8::Value> result;
        if (!script->Run(context).ToLocal(&result) || !result->IsString()) {
            return -12;
        }
        v8::String::Utf8Value message(runtime->isolate, result);
        if (*message == nullptr) {
            return -13;
        }
        const std::string actual(*message, static_cast<size_t>(message.length()));
        return actual == "host callback capability is unavailable" ? 0 : -14;
    }
}

// Test-only probe: one native Node identity must map to distinct wrappers
// in distinct Realms, while repeated wrapping in one Realm preserves identity.
extern "C" int aibe_v8_realm_wrapper_cache_probe(
    void* opaque, uint64_t first_realm_id, uint64_t second_realm_id)
{
    if (opaque == nullptr || first_realm_id == 0 || second_realm_id == 0
        || first_realm_id == second_realm_id) {
        return -1;
    }
    auto* runtime = static_cast<AibeV8Runtime*>(opaque);
    if (std::this_thread::get_id() != runtime->owner_thread) {
        return -7;
    }
    auto first = runtime->realms.find(first_realm_id);
    auto second = runtime->realms.find(second_realm_id);
    if (first == runtime->realms.end() || second == runtime->realms.end()) {
        return -9;
    }

    v8::Isolate::Scope isolate_scope(runtime->isolate);
    v8::HandleScope handle_scope(runtime->isolate);
    auto native_node = std::make_shared<AibeNativeNode>();
    v8::Local<v8::Object> first_wrapper;
    v8::Local<v8::Object> first_wrapper_again;
    v8::Local<v8::Object> second_wrapper;
    {
        auto context = first->second.Get(runtime->isolate);
        v8::Context::Scope context_scope(context);
        if (!wrap_node(runtime, native_node, first_realm_id).ToLocal(&first_wrapper)
            || !wrap_node(runtime, native_node, first_realm_id).ToLocal(&first_wrapper_again)) {
            return -8;
        }
        if (!first_wrapper->StrictEquals(first_wrapper_again)) {
            return -10;
        }
    }
    {
        auto context = second->second.Get(runtime->isolate);
        v8::Context::Scope context_scope(context);
        if (!wrap_node(runtime, native_node, second_realm_id).ToLocal(&second_wrapper)) {
            return -11;
        }
        if (first_wrapper->StrictEquals(second_wrapper)) {
            return -12;
        }
        if (node_wrapper_from(runtime->isolate, first_wrapper) != nullptr) {
            return -13;
        }
        if (node_wrapper_from(runtime->isolate, second_wrapper) == nullptr) {
            return -14;
        }
    }
    return 0;
}

extern "C" int aibe_v8_eval_value_in_realm(
    void* opaque,
    uint64_t realm_id,
    const char* source,
    int* value_tag,
    int* boolean_value,
    double* number_value,
    char* output,
    size_t capacity,
    size_t* output_length,
    uint32_t* error_line,
    uint32_t* error_column)
{
    return aibe_v8_eval_value_for_realm(
        opaque, realm_id, source, value_tag, boolean_value,
        number_value, output, capacity, output_length, error_line, error_column);
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

extern "C" int aibe_v8_cppgc_cycle_probe()
{
    if (!initialize_v8_once("/usr/local/bin/d8")) {
        return -1;
    }

    auto cpp_heap = v8::CppHeap::Create(
        g_v8_platform, v8::CppHeapCreateParams({}));
    if (!cpp_heap) {
        return -2;
    }

    auto allocator = std::unique_ptr<v8::ArrayBuffer::Allocator>(
        v8::ArrayBuffer::Allocator::NewDefaultAllocator());
    if (!allocator) {
        return -3;
    }

    v8::Isolate::CreateParams params;
    params.array_buffer_allocator = allocator.get();
    params.cpp_heap = cpp_heap.release();
    auto* isolate = v8::Isolate::New(params);
    if (isolate == nullptr) {
        return -4;
    }
    isolate->SetMicrotasksPolicy(v8::MicrotasksPolicy::kExplicit);

    const size_t before = g_finalized_cppgc_nodes.load(std::memory_order_relaxed);
    const bool script_ok = run_cppgc_cycle_script(isolate);
    if (script_ok) {
        v8::Isolate::Scope isolate_scope(isolate);
        // LowMemoryNotification requests a full collection without requiring
        // the embedder to enable the JavaScript --expose-gc flag.
        isolate->LowMemoryNotification();
        isolate->GetCppHeap()->FinishSweepingForTesting();
    }

    const size_t after = g_finalized_cppgc_nodes.load(std::memory_order_relaxed);
    isolate->Dispose();
    if (!script_ok) {
        return -5;
    }
    return after > before ? static_cast<int>(after - before) : 0;
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
        for (auto& realm : runtime->realms) {
            realm.second.Reset();
        }
        runtime->realms.clear();
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
