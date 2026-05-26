using System.Text;
using NodalMerge.DotNetHost.Ffi;
using BenchmarkDotNet.Attributes;

[MemoryDiagnoser]
public class HostFfiBenchmarks
{
    private const string RoomId = "bench-room";
    private const string Namespace = "bench";

    private HostFfiClient _ffi = null!;
    private string _noopJson = string.Empty;
    private string _mapSetJson = string.Empty;
    private string _requestServerPackJson = string.Empty;
    private string _blobSet50kbJson = string.Empty;

    [GlobalSetup]
    public void Setup()
    {
        _ffi = new HostFfiClient();

        _noopJson = $"{{\"room_id\":\"{RoomId}\",\"command\":\"Noop\"}}";
        _mapSetJson = BuildMapSetJson("fixed-map-key", "{\"n\":42}");
        _requestServerPackJson = $"{{\"room_id\":\"{RoomId}\",\"command\":{{\"RequestServerPack\":{{\"known_ids\":[]}}}}}}";
        _blobSet50kbJson = BuildBlobSetJson();

        SubmitAndAssertOk($"{{\"room_id\":\"{RoomId}\",\"command\":\"EnsureRoom\"}}");

        // Seed 1k ops once to make request-pack style commands meaningful.
        for (var i = 0; i < 1_000; i++)
        {
            SubmitAndAssertOk(BuildMapSetJson($"seed-{i}", i.ToString()));
        }
    }

    [GlobalCleanup]
    public void Cleanup()
    {
        _ffi.Dispose();
    }

    [Benchmark]
    public int SubmitNoopJson()
    {
        var (status, eventsJson) = _ffi.SubmitCommandJson(_noopJson);
        if (status != AsStatus.Ok)
        {
            throw new InvalidOperationException($"Noop failed: {status}");
        }

        return eventsJson.Length;
    }

    [Benchmark]
    public int SubmitMapSetJson()
    {
        var (status, eventsJson) = _ffi.SubmitCommandJson(_mapSetJson);
        if (status != AsStatus.Ok)
        {
            throw new InvalidOperationException($"MapSet failed: {status}");
        }

        return eventsJson.Length;
    }

    [Benchmark]
    public int SubmitRequestServerPack1kJson()
    {
        var (status, eventsJson) = _ffi.SubmitCommandJson(_requestServerPackJson);
        if (status != AsStatus.Ok)
        {
            throw new InvalidOperationException($"RequestServerPack failed: {status}");
        }

        return eventsJson.Length;
    }

    [Benchmark]
    public int SubmitBlobSet50KbJson()
    {
        var (status, eventsJson) = _ffi.SubmitCommandJson(_blobSet50kbJson);
        if (status != AsStatus.Ok)
        {
            throw new InvalidOperationException($"BlobSet failed: {status}");
        }

        return eventsJson.Length;
    }

    private static string BuildMapSetJson(string key, string valueJson)
    {
        return $"{{\"room_id\":\"{RoomId}\",\"command\":{{\"MapSet\":{{\"namespace\":\"{Namespace}\",\"key\":\"{key}\",\"value\":{valueJson}}}}}}}";
    }

    private static string BuildBlobSetJson()
    {
        var bytes = new byte[50 * 1024];
        for (var i = 0; i < bytes.Length; i++)
        {
            bytes[i] = 7;
        }

        var dataB64 = Convert.ToBase64String(bytes);
        return $"{{\"room_id\":\"{RoomId}\",\"command\":{{\"BlobSet\":{{\"namespace\":\"{Namespace}\",\"hash\":\"blob-50kb-fixed\",\"data_b64\":\"{dataB64}\"}}}}}}";
    }

    private void SubmitAndAssertOk(string commandJson)
    {
        var (status, _) = _ffi.SubmitCommandJson(commandJson);
        if (status != AsStatus.Ok)
        {
            throw new InvalidOperationException($"Setup command failed: {status} | {TrimForLog(commandJson)}");
        }
    }

    private static string TrimForLog(string commandJson)
    {
        if (commandJson.Length <= 180)
        {
            return commandJson;
        }

        return commandJson[..180] + "...";
    }
}
