//! Input Automation - Mouse, Keyboard, UI Automation
//! Low-latency OS-level input for desktop automation

use anyhow::Result;
use serde_json::{json, Value};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};
use std::sync::Mutex;
use once_cell::sync::Lazy;

// Track last known mouse position and time for activity detection
static LAST_MOUSE_CHECK: Lazy<Mutex<(i64, i64, Instant)>> = Lazy::new(|| {
    Mutex::new((0, 0, Instant::now()))
});

/// Check if a password field is currently focused (for screenshot safety)
pub async fn check_password_field() -> Result<Value> {
    let ps_script = r#"
        Add-Type -AssemblyName UIAutomationClient
        $auto = [System.Windows.Automation.AutomationElement]
        $focused = $auto::FocusedElement
        
        if ($null -eq $focused) {
            @{ is_password = $false; element_type = "none"; element_name = "" } | ConvertTo-Json
            return
        }
        
        $controlType = $focused.Current.ControlType.ProgrammaticName
        $name = $focused.Current.Name
        $className = $focused.Current.ClassName
        
        # Check if it's a password field
        $isPassword = $false
        
        # Method 1: Check if it's an Edit control with IsPassword pattern
        try {
            $pattern = $focused.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern)
            # Can't directly check IsPassword, but we can check control patterns
        } catch {}
        
        # Method 2: Check class name for common password indicators
        $passwordClasses = @("PasswordBox", "PASSWORD", "Chrome_RenderWidgetHostHWND")
        $passwordNames = @("password", "passwd", "pwd", "pin", "secret", "credential")
        
        foreach ($cls in $passwordClasses) {
            if ($className -like "*$cls*") { $isPassword = $true; break }
        }
        
        foreach ($pname in $passwordNames) {
            if ($name -like "*$pname*") { $isPassword = $true; break }
        }
        
        # Method 3: Check automation ID
        $autoId = $focused.Current.AutomationId
        foreach ($pname in $passwordNames) {
            if ($autoId -like "*$pname*") { $isPassword = $true; break }
        }
        
        # Method 4: For web browsers, check if the focused element name suggests password
        if ($controlType -eq "ControlType.Edit" -or $controlType -eq "ControlType.Document") {
            # Browser password fields often have specific patterns
            try {
                $value = $focused.GetCurrentPropertyValue([System.Windows.Automation.AutomationElement]::HelpTextProperty)
                foreach ($pname in $passwordNames) {
                    if ($value -like "*$pname*") { $isPassword = $true; break }
                }
            } catch {}
        }
        
        @{
            is_password = $isPassword
            element_type = $controlType -replace 'ControlType.',''
            element_name = $name
            element_class = $className
            automation_id = $autoId
        } | ConvertTo-Json
    "#;
    
    let output = Command::new("powershell")
        .args(["-Command", ps_script])
        .output()?;
    
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: Value = serde_json::from_str(stdout.trim()).unwrap_or(json!({
        "is_password": false,
        "element_type": "unknown"
    }));
    
    Ok(json!({
        "success": true,
        "is_password": parsed["is_password"],
        "element_type": parsed["element_type"],
        "element_name": parsed["element_name"],
        "safe_to_screenshot": !parsed["is_password"].as_bool().unwrap_or(false)
    }))
}

/// Check if user has been active (mouse moved) in last N seconds
pub async fn check_user_activity(args: Value) -> Result<Value> {
    let threshold_secs = args["threshold_secs"].as_u64().unwrap_or(30);
    let samples = args["samples"].as_u64().unwrap_or(3);
    let sample_interval_ms = args["sample_interval_ms"].as_u64().unwrap_or(500);
    
    // Get current mouse position
    let get_pos = || -> (i64, i64) {
        let output = Command::new("powershell")
            .args(["-Command", r#"
                Add-Type -AssemblyName System.Windows.Forms
                $pos = [System.Windows.Forms.Cursor]::Position
                "$($pos.X),$($pos.Y)"
            "#])
            .output()
            .ok();
        
        if let Some(out) = output {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let parts: Vec<&str> = stdout.trim().split(',').collect();
            if parts.len() == 2 {
                let x = parts[0].parse().unwrap_or(0);
                let y = parts[1].parse().unwrap_or(0);
                return (x, y);
            }
        }
        (0, 0)
    };
    
    // Sample mouse positions
    let mut positions = Vec::new();
    for _ in 0..samples {
        positions.push(get_pos());
        thread::sleep(Duration::from_millis(sample_interval_ms));
    }
    
    // Check if mouse moved between samples
    let mut movement_detected = false;
    let mut total_distance: f64 = 0.0;
    
    for i in 1..positions.len() {
        let (x1, y1) = positions[i - 1];
        let (x2, y2) = positions[i];
        let dx = (x2 - x1) as f64;
        let dy = (y2 - y1) as f64;
        let distance = (dx * dx + dy * dy).sqrt();
        total_distance += distance;
        if distance > 5.0 {  // More than 5 pixels = intentional movement
            movement_detected = true;
        }
    }
    
    // Also check against last known position from previous call
    let (last_x, last_y, last_time) = {
        let guard = LAST_MOUSE_CHECK.lock().unwrap();
        (guard.0, guard.1, guard.2)
    };
    
    let current_pos = positions.last().unwrap_or(&(0, 0));
    let time_since_last = last_time.elapsed().as_secs();
    
    // Update stored position
    {
        let mut guard = LAST_MOUSE_CHECK.lock().unwrap();
        *guard = (current_pos.0, current_pos.1, Instant::now());
    }
    
    // Determine if user is active
    let user_active = movement_detected || 
        (time_since_last < threshold_secs && 
         ((current_pos.0 - last_x).abs() > 10 || (current_pos.1 - last_y).abs() > 10));
    
    Ok(json!({
        "success": true,
        "user_active": user_active,
        "movement_detected": movement_detected,
        "total_distance_px": total_distance as i64,
        "samples_taken": samples,
        "threshold_secs": threshold_secs,
        "secs_since_last_check": time_since_last,
        "recommendation": if user_active {
            "User appears active - wait or ask before taking control"
        } else {
            "No activity detected - safe to proceed"
        }
    }))
}

/// Wait for user to stop activity before proceeding
pub async fn wait_for_idle(args: Value) -> Result<Value> {
    let timeout_secs = args["timeout_secs"].as_u64().unwrap_or(60);
    let idle_threshold_secs = args["idle_threshold_secs"].as_u64().unwrap_or(5);
    let check_interval_ms = args["check_interval_ms"].as_u64().unwrap_or(1000);
    
    let start = Instant::now();
    let mut last_pos = (0i64, 0i64);
    let mut idle_start: Option<Instant> = None;
    
    let get_pos = || -> (i64, i64) {
        let output = Command::new("powershell")
            .args(["-Command", r#"
                Add-Type -AssemblyName System.Windows.Forms
                $pos = [System.Windows.Forms.Cursor]::Position
                "$($pos.X),$($pos.Y)"
            "#])
            .output()
            .ok();
        
        if let Some(out) = output {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let parts: Vec<&str> = stdout.trim().split(',').collect();
            if parts.len() == 2 {
                return (parts[0].parse().unwrap_or(0), parts[1].parse().unwrap_or(0));
            }
        }
        (0, 0)
    };
    
    loop {
        if start.elapsed().as_secs() > timeout_secs {
            return Ok(json!({
                "success": false,
                "reason": "timeout",
                "waited_secs": start.elapsed().as_secs(),
                "message": "User remained active - timed out waiting for idle"
            }));
        }
        
        let current_pos = get_pos();
        let moved = (current_pos.0 - last_pos.0).abs() > 5 || 
                    (current_pos.1 - last_pos.1).abs() > 5;
        
        if moved {
            idle_start = None;  // Reset idle timer
            last_pos = current_pos;
        } else if idle_start.is_none() {
            idle_start = Some(Instant::now());
        }
        
        if let Some(idle_since) = idle_start {
            if idle_since.elapsed().as_secs() >= idle_threshold_secs {
                return Ok(json!({
                    "success": true,
                    "idle_secs": idle_since.elapsed().as_secs(),
                    "waited_secs": start.elapsed().as_secs(),
                    "message": "User is idle - safe to proceed"
                }));
            }
        }
        
        thread::sleep(Duration::from_millis(check_interval_ms));
    }
}

/// Get screen resolution
pub async fn get_screen_size() -> Result<Value> {
    let output = Command::new("powershell")
        .args(["-Command", r#"
            Add-Type -AssemblyName System.Windows.Forms
            $screen = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
            @{width=$screen.Width; height=$screen.Height} | ConvertTo-Json
        "#])
        .output()?;
    
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: Value = serde_json::from_str(stdout.trim()).unwrap_or(json!({"width": 1920, "height": 1080}));
    
    Ok(json!({
        "success": true,
        "width": parsed["width"],
        "height": parsed["height"]
    }))
}

/// Get current mouse position
pub async fn get_mouse_position() -> Result<Value> {
    let output = Command::new("powershell")
        .args(["-Command", r#"
            Add-Type -AssemblyName System.Windows.Forms
            $pos = [System.Windows.Forms.Cursor]::Position
            @{x=$pos.X; y=$pos.Y} | ConvertTo-Json
        "#])
        .output()?;
    
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: Value = serde_json::from_str(stdout.trim()).unwrap_or(json!({"x": 0, "y": 0}));
    
    Ok(json!({
        "success": true,
        "x": parsed["x"],
        "y": parsed["y"]
    }))
}

/// Move mouse to position
pub async fn mouse_move(args: Value) -> Result<Value> {
    let x = args["x"].as_i64().unwrap_or(0);
    let y = args["y"].as_i64().unwrap_or(0);
    let duration = args["duration"].as_f64().unwrap_or(0.0);
    
    // For smooth movement, we'd interpolate - for now direct move
    let ps_script = format!(r#"
        Add-Type -AssemblyName System.Windows.Forms
        [System.Windows.Forms.Cursor]::Position = New-Object System.Drawing.Point({}, {})
    "#, x, y);
    
    if duration > 0.0 {
        thread::sleep(Duration::from_secs_f64(duration));
    }
    
    Command::new("powershell")
        .args(["-Command", &ps_script])
        .output()?;
    
    Ok(json!({
        "success": true,
        "moved_to": {"x": x, "y": y}
    }))
}

/// Click mouse at position
pub async fn mouse_click(args: Value) -> Result<Value> {
    let x = args["x"].as_i64();
    let y = args["y"].as_i64();
    let button = args["button"].as_str().unwrap_or("left");
    let clicks = args["clicks"].as_i64().unwrap_or(1);
    
    let _click_type = match button {
        "right" => "RightClick",
        "middle" => "MiddleClick", 
        _ => "Click"
    };
    
    let move_cmd = if let (Some(x), Some(y)) = (x, y) {
        format!(r#"[System.Windows.Forms.Cursor]::Position = New-Object System.Drawing.Point({}, {})"#, x, y)
    } else {
        String::new()
    };
    
    let ps_script = format!(r#"
        Add-Type -AssemblyName System.Windows.Forms
        Add-Type @"
        using System;
        using System.Runtime.InteropServices;
        public class Mouse {{
            [DllImport("user32.dll")]
            public static extern void mouse_event(int dwFlags, int dx, int dy, int dwData, int dwExtraInfo);
            public const int MOUSEEVENTF_LEFTDOWN = 0x02;
            public const int MOUSEEVENTF_LEFTUP = 0x04;
            public const int MOUSEEVENTF_RIGHTDOWN = 0x08;
            public const int MOUSEEVENTF_RIGHTUP = 0x10;
            public const int MOUSEEVENTF_MIDDLEDOWN = 0x20;
            public const int MOUSEEVENTF_MIDDLEUP = 0x40;
        }}
"@
        {}
        for ($i = 0; $i -lt {}; $i++) {{
            {}
            Start-Sleep -Milliseconds 50
        }}
    "#, 
        move_cmd, 
        clicks,
        match button {
            "right" => "[Mouse]::mouse_event([Mouse]::MOUSEEVENTF_RIGHTDOWN, 0, 0, 0, 0); [Mouse]::mouse_event([Mouse]::MOUSEEVENTF_RIGHTUP, 0, 0, 0, 0)",
            "middle" => "[Mouse]::mouse_event([Mouse]::MOUSEEVENTF_MIDDLEDOWN, 0, 0, 0, 0); [Mouse]::mouse_event([Mouse]::MOUSEEVENTF_MIDDLEUP, 0, 0, 0, 0)",
            _ => "[Mouse]::mouse_event([Mouse]::MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0); [Mouse]::mouse_event([Mouse]::MOUSEEVENTF_LEFTUP, 0, 0, 0, 0)"
        }
    );
    
    Command::new("powershell")
        .args(["-Command", &ps_script])
        .output()?;
    
    Ok(json!({
        "success": true,
        "button": button,
        "clicks": clicks,
        "position": {"x": x, "y": y}
    }))
}

/// Drag mouse to position
pub async fn mouse_drag(args: Value) -> Result<Value> {
    let x = args["x"].as_i64().unwrap_or(0);
    let y = args["y"].as_i64().unwrap_or(0);
    let duration = args["duration"].as_f64().unwrap_or(0.5);
    
    let ps_script = format!(r#"
        Add-Type @"
        using System;
        using System.Runtime.InteropServices;
        public class Mouse {{
            [DllImport("user32.dll")]
            public static extern void mouse_event(int dwFlags, int dx, int dy, int dwData, int dwExtraInfo);
            public const int MOUSEEVENTF_LEFTDOWN = 0x02;
            public const int MOUSEEVENTF_LEFTUP = 0x04;
            public const int MOUSEEVENTF_MOVE = 0x01;
            public const int MOUSEEVENTF_ABSOLUTE = 0x8000;
        }}
"@
        Add-Type -AssemblyName System.Windows.Forms
        [Mouse]::mouse_event([Mouse]::MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0)
        Start-Sleep -Milliseconds {}
        [System.Windows.Forms.Cursor]::Position = New-Object System.Drawing.Point({}, {})
        Start-Sleep -Milliseconds 50
        [Mouse]::mouse_event([Mouse]::MOUSEEVENTF_LEFTUP, 0, 0, 0, 0)
    "#, (duration * 1000.0) as i64, x, y);
    
    Command::new("powershell")
        .args(["-Command", &ps_script])
        .output()?;
    
    Ok(json!({
        "success": true,
        "dragged_to": {"x": x, "y": y}
    }))
}

/// Scroll mouse wheel
pub async fn mouse_scroll(args: Value) -> Result<Value> {
    let clicks = args["clicks"].as_i64().unwrap_or(3);
    let wheel_delta = clicks * 120; // Standard wheel delta
    
    let ps_script = format!(r#"
        Add-Type @"
        using System;
        using System.Runtime.InteropServices;
        public class Mouse {{
            [DllImport("user32.dll")]
            public static extern void mouse_event(int dwFlags, int dx, int dy, int dwData, int dwExtraInfo);
            public const int MOUSEEVENTF_WHEEL = 0x0800;
        }}
"@
        [Mouse]::mouse_event([Mouse]::MOUSEEVENTF_WHEEL, 0, 0, {}, 0)
    "#, wheel_delta);
    
    Command::new("powershell")
        .args(["-Command", &ps_script])
        .output()?;
    
    Ok(json!({
        "success": true,
        "scrolled": clicks
    }))
}

/// Type text with keyboard
pub async fn keyboard_type(args: Value) -> Result<Value> {
    let text = args["text"].as_str().unwrap_or("");
    let interval = args["interval"].as_f64().unwrap_or(0.01);
    
    let escaped = text.replace("\"", "`\"").replace("'", "''");
    let ps_script = format!(r#"
        Add-Type -AssemblyName System.Windows.Forms
        [System.Windows.Forms.SendKeys]::SendWait("{}")
    "#, escaped.replace("{", "{{").replace("}", "}}").replace("+", "{{+}}").replace("^", "{{^}}").replace("%", "{{%}}"));
    
    if interval > 0.0 {
        thread::sleep(Duration::from_secs_f64(interval * text.len() as f64));
    }
    
    Command::new("powershell")
        .args(["-Command", &ps_script])
        .output()?;
    
    Ok(json!({
        "success": true,
        "typed": text.len()
    }))
}

/// Press a key
pub async fn keyboard_press(args: Value) -> Result<Value> {
    let key = args["key"].as_str().unwrap_or("enter");
    let presses = args["presses"].as_i64().unwrap_or(1);
    
    let sendkey = match key.to_lowercase().as_str() {
        "enter" | "return" => "{ENTER}",
        "tab" => "{TAB}",
        "esc" | "escape" => "{ESC}",
        "backspace" | "bs" => "{BACKSPACE}",
        "delete" | "del" => "{DELETE}",
        "up" => "{UP}",
        "down" => "{DOWN}",
        "left" => "{LEFT}",
        "right" => "{RIGHT}",
        "home" => "{HOME}",
        "end" => "{END}",
        "pageup" | "pgup" => "{PGUP}",
        "pagedown" | "pgdn" => "{PGDN}",
        "f1" => "{F1}", "f2" => "{F2}", "f3" => "{F3}", "f4" => "{F4}",
        "f5" => "{F5}", "f6" => "{F6}", "f7" => "{F7}", "f8" => "{F8}",
        "f9" => "{F9}", "f10" => "{F10}", "f11" => "{F11}", "f12" => "{F12}",
        "space" => " ",
        _ => key
    };
    
    let repeated = sendkey.repeat(presses as usize);
    let ps_script = format!(r#"
        Add-Type -AssemblyName System.Windows.Forms
        [System.Windows.Forms.SendKeys]::SendWait("{}")
    "#, repeated);
    
    Command::new("powershell")
        .args(["-Command", &ps_script])
        .output()?;
    
    Ok(json!({
        "success": true,
        "key": key,
        "presses": presses
    }))
}

/// Press hotkey combination
pub async fn keyboard_hotkey(args: Value) -> Result<Value> {
    let keys = args["keys"].as_str().unwrap_or("");
    
    // Parse keys like "ctrl+c", "alt+tab", "ctrl+shift+s"
    let parts: Vec<&str> = keys.split('+').collect();
    let mut modifiers = String::new();
    let mut main_key = String::new();
    
    for part in &parts {
        let lower = part.to_lowercase();
        match lower.as_str() {
            "ctrl" | "control" => modifiers.push('^'),
            "alt" => modifiers.push('%'),
            "shift" => modifiers.push('+'),
            "win" | "windows" => {
                // Windows key needs special handling
                let ps_script = r#"
                    Add-Type @"
                    using System;
                    using System.Runtime.InteropServices;
                    public class Keyboard {
                        [DllImport("user32.dll")]
                        public static extern void keybd_event(byte bVk, byte bScan, int dwFlags, int dwExtraInfo);
                        public const int KEYEVENTF_KEYUP = 0x02;
                        public const byte VK_LWIN = 0x5B;
                    }
"@
                    [Keyboard]::keybd_event([Keyboard]::VK_LWIN, 0, 0, 0)
                    Start-Sleep -Milliseconds 50
                    [Keyboard]::keybd_event([Keyboard]::VK_LWIN, 0, [Keyboard]::KEYEVENTF_KEYUP, 0)
                "#;
                Command::new("powershell").args(["-Command", ps_script]).output()?;
                return Ok(json!({"success": true, "keys": keys}));
            }
            _ => main_key = lower
        }
    }
    
    let sendkey = match main_key.as_str() {
        "a" => "a", "b" => "b", "c" => "c", "d" => "d", "e" => "e",
        "f" => "f", "g" => "g", "h" => "h", "i" => "i", "j" => "j",
        "k" => "k", "l" => "l", "m" => "m", "n" => "n", "o" => "o",
        "p" => "p", "q" => "q", "r" => "r", "s" => "s", "t" => "t",
        "u" => "u", "v" => "v", "w" => "w", "x" => "x", "y" => "y", "z" => "z",
        "tab" => "{TAB}",
        "enter" => "{ENTER}",
        "esc" | "escape" => "{ESC}",
        "f1" => "{F1}", "f2" => "{F2}", "f3" => "{F3}", "f4" => "{F4}",
        "f5" => "{F5}", "f6" => "{F6}", "f7" => "{F7}", "f8" => "{F8}",
        "f9" => "{F9}", "f10" => "{F10}", "f11" => "{F11}", "f12" => "{F12}",
        _ => &main_key
    };
    
    let combo = format!("{}{}", modifiers, sendkey);
    let ps_script = format!(r#"
        Add-Type -AssemblyName System.Windows.Forms
        [System.Windows.Forms.SendKeys]::SendWait("{}")
    "#, combo);
    
    Command::new("powershell")
        .args(["-Command", &ps_script])
        .output()?;
    
    Ok(json!({
        "success": true,
        "keys": keys
    }))
}

/// Get UI elements from foreground window (UI Automation)
pub async fn uia_state(args: Value) -> Result<Value> {
    let max_depth = args["max_depth"].as_i64().unwrap_or(3);
    let include_invisible = args["include_invisible"].as_bool().unwrap_or(false);
    
    let ps_script = format!(r#"
        Add-Type -AssemblyName UIAutomationClient
        Add-Type -AssemblyName UIAutomationTypes
        
        $auto = [System.Windows.Automation.AutomationElement]
        $root = $auto::FocusedElement
        if ($null -eq $root) {{ $root = $auto::RootElement }}
        
        $window = $root
        while ($window.Current.ControlType -ne [System.Windows.Automation.ControlType]::Window -and $null -ne $window.CachedParent) {{
            $parent = [System.Windows.Automation.TreeWalker]::RawViewWalker.GetParent($window)
            if ($null -eq $parent) {{ break }}
            $window = $parent
        }}
        
        $elements = @()
        $condition = [System.Windows.Automation.Condition]::TrueCondition
        
        function Get-Elements($elem, $depth) {{
            if ($depth -gt {}) {{ return }}
            
            $children = $elem.FindAll([System.Windows.Automation.TreeScope]::Children, $condition)
            foreach ($child in $children) {{
                try {{
                    $rect = $child.Current.BoundingRectangle
                    if ({} -or ($rect.Width -gt 0 -and $rect.Height -gt 0)) {{
                        $script:elements += @{{
                            type = $child.Current.ControlType.ProgrammaticName -replace 'ControlType.',''
                            name = $child.Current.Name
                            loc = @([int]($rect.X + $rect.Width/2), [int]($rect.Y + $rect.Height/2))
                            rect = @([int]$rect.X, [int]$rect.Y, [int]($rect.X + $rect.Width), [int]($rect.Y + $rect.Height))
                            interactive = $child.Current.IsEnabled -and $child.Current.IsKeyboardFocusable
                        }}
                    }}
                    Get-Elements $child ($depth + 1)
                }} catch {{}}
            }}
        }}
        
        Get-Elements $window 0
        @{{
            window = $window.Current.Name
            element_count = $elements.Count
            elements = $elements
        }} | ConvertTo-Json -Depth 5 -Compress
    "#, max_depth, if include_invisible { "$true" } else { "$false" });
    
    let output = Command::new("powershell")
        .args(["-Command", &ps_script])
        .output()?;
    
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: Value = serde_json::from_str(stdout.trim()).unwrap_or(json!({
        "window": "unknown",
        "element_count": 0,
        "elements": []
    }));
    
    Ok(json!({
        "success": true,
        "window": parsed["window"],
        "element_count": parsed["element_count"],
        "elements": parsed["elements"]
    }))
}

/// List all visible windows
pub async fn uia_windows() -> Result<Value> {
    let ps_script = r#"
        Add-Type -AssemblyName UIAutomationClient
        $auto = [System.Windows.Automation.AutomationElement]
        $condition = New-Object System.Windows.Automation.PropertyCondition(
            [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
            [System.Windows.Automation.ControlType]::Window
        )
        
        $windows = $auto::RootElement.FindAll([System.Windows.Automation.TreeScope]::Children, $condition)
        $result = @()
        
        foreach ($win in $windows) {
            try {
                $rect = $win.Current.BoundingRectangle
                if ($rect.Width -gt 0 -and $rect.Height -gt 0 -and $win.Current.Name -ne '') {
                    $result += @{
                        title = $win.Current.Name
                        class = $win.Current.ClassName
                        loc = @([int]($rect.X + $rect.Width/2), [int]($rect.Y + $rect.Height/2))
                        rect = @([int]$rect.X, [int]$rect.Y, [int]($rect.X + $rect.Width), [int]($rect.Y + $rect.Height))
                    }
                }
            } catch {}
        }
        
        $result | ConvertTo-Json -Depth 3 -Compress
    "#;
    
    let output = Command::new("powershell")
        .args(["-Command", ps_script])
        .output()?;
    
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: Value = serde_json::from_str(stdout.trim()).unwrap_or(json!([]));
    
    Ok(json!({
        "success": true,
        "windows": parsed
    }))
}

/// Find UI element by name or type
pub async fn uia_find(args: Value) -> Result<Value> {
    let name = args["name"].as_str().unwrap_or("");
    let control_type = args["control_type"].as_str().unwrap_or("");
    
    let name_filter = if !name.is_empty() {
        format!(r#"$_.name -like '*{}*'"#, name)
    } else {
        "$true".to_string()
    };
    
    let type_filter = if !control_type.is_empty() {
        format!(r#"$_.type -eq '{}'"#, control_type)
    } else {
        "$true".to_string()
    };
    
    let ps_script = format!(r#"
        Add-Type -AssemblyName UIAutomationClient
        $auto = [System.Windows.Automation.AutomationElement]
        $condition = [System.Windows.Automation.Condition]::TrueCondition
        
        $focused = $auto::FocusedElement
        $window = $focused
        while ($window.Current.ControlType -ne [System.Windows.Automation.ControlType]::Window) {{
            $parent = [System.Windows.Automation.TreeWalker]::RawViewWalker.GetParent($window)
            if ($null -eq $parent) {{ break }}
            $window = $parent
        }}
        
        $all = $window.FindAll([System.Windows.Automation.TreeScope]::Descendants, $condition)
        $matches = @()
        
        foreach ($elem in $all) {{
            try {{
                $rect = $elem.Current.BoundingRectangle
                if ($rect.Width -gt 0 -and $rect.Height -gt 0) {{
                    $item = @{{
                        type = $elem.Current.ControlType.ProgrammaticName -replace 'ControlType.',''
                        name = $elem.Current.Name
                        loc = @([int]($rect.X + $rect.Width/2), [int]($rect.Y + $rect.Height/2))
                    }}
                    if (({}) -and ({})) {{
                        $matches += $item
                    }}
                }}
            }} catch {{}}
        }}
        
        @{{ matches = $matches }} | ConvertTo-Json -Depth 3 -Compress
    "#, name_filter, type_filter);
    
    let output = Command::new("powershell")
        .args(["-Command", &ps_script])
        .output()?;
    
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: Value = serde_json::from_str(stdout.trim()).unwrap_or(json!({"matches": []}));
    
    Ok(json!({
        "success": true,
        "query": {"name": name, "type": control_type},
        "matches": parsed["matches"]
    }))
}
