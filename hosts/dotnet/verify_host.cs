using System;
using System.Net.Http;
using System.Net.WebSockets;
using System.Text;
using System.Text.Json;
using System.Threading;
using System.Threading.Tasks;
using System.Diagnostics;

class Program {
    static async Task Main(string[] args) {
        var baseUrl = "http://127.0.0.1:8787";
        var wsUrl = "ws://127.0.0.1:8787/ws/runtime";
        
        Console.WriteLine("Starting Host...");
        var processStartInfo = new ProcessStartInfo {
            FileName = "dotnet",
            Arguments = "run --project src/NodalMerge.DotNetHost/NodalMerge.DotNetHost.csproj",
            UseShellExecute = false,
            CreateNoWindow = true,
            RedirectStandardOutput = true,
            RedirectStandardError = true
        };
        processStartInfo.EnvironmentVariables["ASPNETCORE_URLS"] = baseUrl;
        
        using var process = Process.Start(processStartInfo);
        
        try {
            using var client = new HttpClient();
            var sw = Stopwatch.StartNew();
            bool ready = false;
            while (sw.Elapsed.TotalSeconds < 30) {
                try {
                    var response = await client.GetAsync($"{baseUrl}/ffi/abi-version");
                    if (response.IsSuccessStatusCode) {
                        ready = true;
                        break;
                    }
                } catch {}
                await Task.Delay(1000);
            }
            
            if (!ready) {
                Console.WriteLine("Server failed to respond within 30s");
                process.Kill();
                Environment.Exit(1);
            }
            
            Console.WriteLine("Server ready. Connecting WebSocket...");
            using var ws = new ClientWebSocket();
            await ws.ConnectAsync(new Uri(wsUrl), CancellationToken.None);
            
            var hello = JsonSerializer.Serialize(new { type = "hello" });
            var noop = JsonSerializer.Serialize(new { type = "noop" });
            
            await ws.SendAsync(new ArraySegment<byte>(Encoding.UTF8.GetBytes(hello)), WebSocketMessageType.Text, true, CancellationToken.None);
            await ws.SendAsync(new ArraySegment<byte>(Encoding.UTF8.GetBytes(noop)), WebSocketMessageType.Text, true, CancellationToken.None);
            
            var buffer = new byte[1024 * 4];
            var result = await ws.ReceiveAsync(new ArraySegment<byte>(buffer), CancellationToken.None);
            var responseText = Encoding.UTF8.GetString(buffer, 0, result.Count);
            
            Console.WriteLine($"Received: {responseText}");
            
            if (responseText.Contains("noop-ack")) {
                Console.WriteLine("Verification SUCCESS");
            } else {
                Console.WriteLine("Verification FAILED: noop-ack not found");
                Environment.Exit(1);
            }
            
            await ws.CloseAsync(WebSocketCloseStatus.NormalClosure, "Done", CancellationToken.None);
        } finally {
            if (!process.HasExited) {
                process.Kill();
            }
        }
    }
}

