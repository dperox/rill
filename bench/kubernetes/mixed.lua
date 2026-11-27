-- Validate every completed response, independent of which path sent it.
local threads = {}
local expected = {
    [string.rep("S", 4096)] = "static",
    [string.rep("A", 4096)] = "api",
    [string.rep("D", 262144)] = "download",
}
local paths = {
    static = "/static.bin",
    api = "/api/payload.bin",
    download = "/downloads/payload.bin",
}
local requests = {}
local position = 0
checked = 0
invalid = 0
static_count = 0
api_count = 0
download_count = 0

function setup(thread)
    threads[#threads + 1] = thread
end

function init(args)
    local profile = args[1] or "mixed"
    if profile == "mixed" then
        for _, name in ipairs({"static", "static", "api", "static", "static",
                               "download", "static", "api", "static", "static"}) do
            requests[#requests + 1] = wrk.format("GET", paths[name])
        end
    else
        assert(paths[profile], "unknown request profile")
        requests[1] = wrk.format("GET", paths[profile])
    end
end

function request()
    position = position % #requests + 1
    return requests[position]
end

function response(status, headers, body)
    checked = checked + 1
    local kind = expected[body]
    if status ~= 200 or not kind then
        invalid = invalid + 1
    elseif kind == "static" then
        static_count = static_count + 1
    elseif kind == "api" then
        api_count = api_count + 1
    else
        download_count = download_count + 1
    end
end

function done(summary, latency)
    local counts = {checked = 0, invalid = 0, static_count = 0, api_count = 0, download_count = 0}
    for _, thread in ipairs(threads) do
        for field, _ in pairs(counts) do
            counts[field] = counts[field] + thread:get(field)
        end
    end
    local errors = summary.errors
    print(string.format("RESULT %d %.3f %.2f %.3f %.3f %d %d %d %d %d %d %d %d %d %d",
        summary.requests, summary.duration / 1e6, summary.requests * 1e6 / summary.duration,
        latency:percentile(95) / 1e3, latency:percentile(99) / 1e3,
        counts.checked, counts.invalid, counts.static_count, counts.api_count, counts.download_count,
        errors.connect, errors.read, errors.write, errors.timeout, errors.status))
end
