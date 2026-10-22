param([string]$Executable='apps/yydbmanager/target/debug/yydbmanager.exe', [string]$OutputDirectory='target/yydbmanager-shell', [string]$PlanFile='', [switch]$HelpOnly, [switch]$ForcePlan)
$ErrorActionPreference='Stop'
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class DbShellTest {
 public static void SelectDocumentTab(IntPtr parent,IntPtr tabs,int index) {
   // TCM_SETCURFOCUS lets the native tab send its own selection notifications.
   SendMessage(tabs,0x1330,(IntPtr)index,IntPtr.Zero);
 }

 [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
 private delegate bool WindowCallback(IntPtr h,IntPtr p);
 [DllImport("user32.dll")] private static extern bool EnumWindows(WindowCallback callback,IntPtr p);
 [DllImport("user32.dll")] private static extern uint GetWindowThreadProcessId(IntPtr h,out uint id);
 public static IntPtr FindProcessWindow(uint id) {IntPtr found=IntPtr.Zero;EnumWindows((h,p)=>{uint pid;GetWindowThreadProcessId(h,out pid);if(pid==id && GetDlgItem(h,202)!=IntPtr.Zero){found=h;return false;}return true;},IntPtr.Zero);return found;}
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] private static extern int GetClassName(IntPtr h,System.Text.StringBuilder s,int n);
 public static IntPtr FindChild(IntPtr parent,string cls) {for(var h=GetWindow(parent,5);h!=IntPtr.Zero;h=GetWindow(h,2)){var s=new System.Text.StringBuilder(256);GetClassName(h,s,256);if(s.ToString().Equals(cls,StringComparison.OrdinalIgnoreCase))return h;}return IntPtr.Zero;}
 public static string Describe(uint id) {var s=new System.Text.StringBuilder();EnumWindows((h,p)=>{uint pid;GetWindowThreadProcessId(h,out pid);if(pid==id){var c=new System.Text.StringBuilder(256);var t=new System.Text.StringBuilder(512);GetClassName(h,c,256);GetWindowText(h,t,512);s.AppendLine(h+" "+c+" "+t);for(var ch=GetWindow(h,5);ch!=IntPtr.Zero;ch=GetWindow(ch,2)){var ct=new System.Text.StringBuilder(1024);GetWindowText(ch,ct,1024);s.AppendLine(" child: "+ct);}}return true;},IntPtr.Zero);return s.ToString();}
 [StructLayout(LayoutKind.Sequential)] public struct Rect {public int left,top,right,bottom;}
 [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h,int id);
 [DllImport("user32.dll")] public static extern IntPtr GetWindow(IntPtr h,uint cmd);
 [DllImport("user32.dll")] public static extern IntPtr GetMenu(IntPtr h);
 [DllImport("user32.dll")] public static extern int GetMenuItemCount(IntPtr h);
 [DllImport("user32.dll")] public static extern IntPtr GetSubMenu(IntPtr h,int index);
 [DllImport("user32.dll")] public static extern uint GetMenuItemID(IntPtr h,int index);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] public static extern bool SetWindowText(IntPtr h,string value);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h,System.Text.StringBuilder s,int n);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] public static extern IntPtr FindWindow(string cls,string title);
 [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h,uint msg,IntPtr w,IntPtr l);
 [DllImport("user32.dll",EntryPoint="SendMessageW",CharSet=CharSet.Unicode)] public static extern IntPtr ReadMessage(IntPtr h,uint msg,IntPtr w,System.Text.StringBuilder l);
 [DllImport("user32.dll",EntryPoint="SendMessageW",CharSet=CharSet.Unicode)] public static extern IntPtr TextMessage(IntPtr h,uint msg,IntPtr w,string l);
 [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h,uint msg,IntPtr w,IntPtr l);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] public static extern bool SendNotifyMessage(IntPtr h,uint msg,IntPtr w,IntPtr l);
 [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h,int cmd);
 [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h,IntPtr z,int x,int y,int w,int t,uint flags);
 [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h,out Rect r);
 [DllImport("user32.dll",EntryPoint="GetClassLongPtrW")] public static extern IntPtr GetClassLongPtr(IntPtr h,int n);
 [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr h);
 [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
 [DllImport("user32.dll")] public static extern bool IsWindowEnabled(IntPtr h);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] public static extern int GetMenuString(IntPtr h,uint item,System.Text.StringBuilder text,int count,uint flags);
 [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h,IntPtr dc,uint flags);
}
'@
New-Item -ItemType Directory -Force $OutputDirectory | Out-Null
$taskOutput=(Resolve-Path $OutputDirectory).Path
$taskOldConfig=$env:YYDBMANAGER_CONNECTIONS_FILE
$env:YYDBMANAGER_CONNECTIONS_FILE=Join-Path $taskOutput "connections-$([Guid]::NewGuid()).dat"
$taskTarget=Join-Path $taskOutput 'shell.sqlite'
$taskProcess=$null
$taskOldClipboard=Get-Clipboard -Raw
$taskStart=@{FilePath=$Executable;WindowStyle='Hidden';PassThru=$true}
if($PlanFile){$taskStart.ArgumentList=@('"'+(Resolve-Path $PlanFile).Path+'"')}
if($ForcePlan){$taskStart.ArgumentList=@('--open-plan',('"'+(Resolve-Path $PlanFile).Path+'"'))}
$taskOldDpi=[DbShellTest]::SetThreadDpiAwarenessContext([IntPtr]::new(-4))
function Wait-Frame {
    for($i=0;$i -lt 100;$i++){ $taskProcess.Refresh(); $taskFrame=[DbShellTest]::FindProcessWindow($taskProcess.Id); if($taskFrame -ne [IntPtr]::Zero -and [DbShellTest]::SendMessage($taskFrame,0x802A,0,0).ToInt64() -eq 1){return $taskFrame};if($taskProcess.HasExited){throw 'App exited before showing a window'};Start-Sleep -Milliseconds 50 }
    throw 'Window did not appear'
}
function Snapshot([IntPtr]$Window,[string]$Name) {
    Start-Sleep -Milliseconds 250
    $taskRect=[DbShellTest+Rect]::new(); [DbShellTest]::GetWindowRect($Window,[ref]$taskRect)|Out-Null
    $taskImage=[Drawing.Bitmap]::new($taskRect.right-$taskRect.left,$taskRect.bottom-$taskRect.top)
    $taskGraphics=[Drawing.Graphics]::FromImage($taskImage)
    $taskDC=$taskGraphics.GetHdc()
    try {if(![DbShellTest]::PrintWindow($Window,$taskDC,2)){throw 'PrintWindow failed'}} finally {$taskGraphics.ReleaseHdc($taskDC)}
    $taskImage.Save((Join-Path $taskOutput "$Name.png"),[Drawing.Imaging.ImageFormat]::Png)
    $taskGraphics.Dispose();$taskImage.Dispose()
}
function Wait-VisibleDialog([string]$Class,[string]$Title) {
    for($i=0;$i -lt 100;$i++) {
        $taskHandle=[DbShellTest]::FindWindow($Class,$Title)
        if($taskHandle -ne [IntPtr]::Zero -and [DbShellTest]::IsWindowVisible($taskHandle)){return $taskHandle}
        Start-Sleep -Milliseconds 20
    }
    throw "Dialog did not open: $Title`n$([DbShellTest]::Describe($taskProcess.Id))"
}
function Wait-DialogClosed([IntPtr]$Dialog) {
    for($i=0;$i -lt 200 -and [DbShellTest]::IsWindow($Dialog);$i++){Start-Sleep -Milliseconds 20}
    if([DbShellTest]::IsWindow($Dialog)){throw "Dialog remained open: $Dialog`n$([DbShellTest]::Describe($taskProcess.Id))"}
    for($i=0;$i -lt 100 -and ![DbShellTest]::IsWindowEnabled($taskWindow);$i++){Start-Sleep -Milliseconds 20}
}
try {
    $taskProcess=Start-Process @taskStart
    $taskWindow=Wait-Frame
    if([DbShellTest]::GetClassLongPtr($taskWindow,-14) -eq [IntPtr]::Zero){throw 'Application icon is missing'}
    if([DbShellTest]::IsWindowVisible([DbShellTest]::GetDlgItem($taskWindow,118))){throw 'Result tabs are visible before executing SQL'}

    $taskSetupTabs=[DbShellTest]::GetDlgItem($taskWindow,117)
    $taskSetupBefore=[DbShellTest]::SendMessage($taskSetupTabs,0x1304,0,0).ToInt64()
    [DbShellTest]::SendNotifyMessage($taskWindow,0x111,126,0)|Out-Null
    for($i=0;$i -lt 100 -and [DbShellTest]::SendMessage($taskSetupTabs,0x1304,0,0).ToInt64() -eq $taskSetupBefore;$i++){Start-Sleep -Milliseconds 20}
    if([DbShellTest]::SendMessage($taskSetupTabs,0x1304,0,0).ToInt64() -ne $taskSetupBefore+1){throw 'Db2 setup SQL did not open in a new tab'}

    if([DbShellTest]::GetMenuItemCount([DbShellTest]::GetMenu($taskWindow)) -ne 6){throw 'Expected six top-level menus'}
    $taskHelpTopics=@(
        @{Id=127;Title='yydbmanagerの使い方';Needle='接続先を登録して接続する'},
        @{Id=128;Title='Oracleの権限と実測取得の準備';Needle='GRANT SELECT ON SYS.V_$SESSION'},
        @{Id=129;Title='Db2の権限と実測収集の準備';Needle='CREATE EVENT MONITOR YYDBMANAGER_ACTIVITY'},
        @{Id=130;Title='実行計画の取得と読み方';Needle='全starts'},
        @{Id=131;Title='実測取得のトラブルシューティング';Needle='自動再実行はしません'},
        @{Id=132;Title='ERモデルYAMLファイル仕様';Needle='format_version'},
        @{Id=133;Title='DB別の性能分析ビュー';Needle='累積値は差分で比較する'},
        @{Id=134;Title='実行計画ファイルの形式と取得方法';Needle='Db2の8列DEL'}
    )
    foreach($taskTopic in $taskHelpTopics) {
        if($taskTopic.Id -eq 127){[DbShellTest]::PostMessage($taskWindow,0x100,0x70,0)|Out-Null}
        else {[DbShellTest]::SendNotifyMessage($taskWindow,0x111,$taskTopic.Id,0)|Out-Null}
        $taskHelp=[IntPtr]::Zero
        for($i=0;$i -lt 100;$i++){$taskHelp=[DbShellTest]::FindWindow('YYSeriesHelp','yydbmanager ヘルプ');if($taskHelp -ne [IntPtr]::Zero -and [DbShellTest]::IsWindowVisible($taskHelp)){break};Start-Sleep -Milliseconds 20}
        if($taskHelp -eq [IntPtr]::Zero){throw "Help did not open: $($taskTopic.Title)"}
        $taskHelpEdit=[DbShellTest]::FindChild($taskHelp,'Edit')
        $taskHelpLength=[DbShellTest]::SendMessage($taskHelpEdit,0xE,0,0).ToInt32()+1
        $taskHelpText=[Text.StringBuilder]::new($taskHelpLength)
        [DbShellTest]::ReadMessage($taskHelpEdit,0xD,$taskHelpLength,$taskHelpText)|Out-Null
        if(!$taskHelpText.ToString().Contains($taskTopic.Needle) -or $taskHelpText.ToString() -match '\{\{(?:DB2_SETUP_SQL|DB2_PLAN_EXPORT|ORACLE_PLAN_EXPORT)\}\}'){throw "Help content missing: $($taskTopic.Title)"}
        if([DbShellTest]::SendMessage($taskHelpEdit,0xBA,0,0).ToInt64() -lt 10){throw "Help multiline display failed: topic=$($taskTopic.Id), length=$taskHelpLength, lines=$([DbShellTest]::SendMessage($taskHelpEdit,0xBA,0,0))"}
        if($taskTopic.Id -in @(132,133,134) -and !$env:YY_HELP_TEXT_ONLY){
            for($i=0;$i -lt 100 -and [DbShellTest]::IsWindowVisible($taskHelpEdit);$i++){Start-Sleep -Milliseconds 50}
        }
        Snapshot $taskHelp "help-$($taskTopic.Id)"
        [DbShellTest]::PostMessage($taskHelp,0x10,0,0)|Out-Null
        Start-Sleep -Milliseconds 40
    }
    if($HelpOnly){ 'Embedded Markdown help topics and offline text fallback OK'; return }

    foreach($taskObsoleteControl in @(206,207,208)) {
        if([DbShellTest]::GetDlgItem($taskWindow,$taskObsoleteControl) -ne [IntPtr]::Zero){throw 'Transfer setting remains on the main window'}
    }
    foreach($taskTransfer in @(@{Id=110;Title='データをインポート'},@{Id=109;Title='結果をエクスポート'})) {
        [DbShellTest]::SendNotifyMessage($taskWindow,0x111,$taskTransfer.Id,0)|Out-Null
        $taskTransferDialog=[IntPtr]::Zero
        for($i=0;$i -lt 100;$i++){$taskTransferDialog=[DbShellTest]::FindWindow('YYDbTransfer',$taskTransfer.Title);if($taskTransferDialog -ne [IntPtr]::Zero -and [DbShellTest]::IsWindowVisible($taskTransferDialog)){break};Start-Sleep -Milliseconds 20}
        if($taskTransferDialog -eq [IntPtr]::Zero){throw 'Transfer settings dialog did not open'}
        $taskFormat=[DbShellTest]::GetDlgItem($taskTransferDialog,101)
        $taskDelimiter=[DbShellTest]::GetDlgItem($taskTransferDialog,102)
        $taskCharset=[DbShellTest]::GetDlgItem($taskTransferDialog,103)
        if([DbShellTest]::IsWindowEnabled($taskDelimiter) -or [DbShellTest]::IsWindowEnabled($taskCharset)){throw 'CSV exposes inapplicable options'}
        foreach($taskIndex in @(2,3,4,0,4)) {
            [DbShellTest]::SendMessage($taskFormat,0x14E,$taskIndex,0)|Out-Null
            [DbShellTest]::SendMessage($taskTransferDialog,0x111,(101 -bor (1 -shl 16)),$taskFormat)|Out-Null
            if([DbShellTest]::IsWindowEnabled($taskDelimiter) -ne ($taskIndex -eq 2) -or [DbShellTest]::IsWindowEnabled($taskCharset) -ne ($taskIndex -eq 4)){throw 'Transfer option availability does not match format'}
            if($taskIndex -eq 4){
                $taskCharsetValue=[Text.StringBuilder]::new(256)
                [DbShellTest]::ReadMessage($taskCharset,0xD,256,$taskCharsetValue)|Out-Null
                if($taskCharsetValue.ToString() -ne 'MS932'){throw 'Fixed-length character set was not retained'}
            }
        }
        Snapshot $taskTransferDialog "transfer-$($taskTransfer.Id)"
        [DbShellTest]::SendNotifyMessage($taskTransferDialog,0x111,2,0)|Out-Null
        for($i=0;$i -lt 100 -and ![DbShellTest]::IsWindowEnabled($taskWindow);$i++){Start-Sleep -Milliseconds 20}
        if(![DbShellTest]::IsWindowEnabled($taskWindow)){throw 'Cancel did not restore the main window'}
    }

    if([IO.Path]::GetExtension($PlanFile) -in @('.yyer','.yaml','.yml')) {
        [DbShellTest]::SendNotifyMessage($taskWindow,0x111,9101,0)|Out-Null
        $taskChoice=[IntPtr]::Zero
        for($i=0;$i -lt 100;$i++){$taskChoice=[DbShellTest]::FindWindow('YYDbERChoice','ER図に表示するテーブルとビュー');if($taskChoice -ne [IntPtr]::Zero -and [DbShellTest]::GetDlgItem($taskChoice,2) -ne [IntPtr]::Zero){break};Start-Sleep -Milliseconds 30}
        if($taskChoice -eq [IntPtr]::Zero){throw 'ER table selection did not open'}
        $taskList=[DbShellTest]::GetDlgItem($taskChoice,10)
        if([DbShellTest]::SendMessage($taskList,0x18B,0,0).ToInt64() -lt 2){throw 'ER table selection has no entities'}
        [DbShellTest]::SendNotifyMessage($taskChoice,0x111,4,0)|Out-Null
        Start-Sleep -Milliseconds 60
        if([DbShellTest]::SendMessage($taskList,0x190,0,0).ToInt64() -ne 0){throw 'ER clear selection failed'}
        [DbShellTest]::SendNotifyMessage($taskChoice,0x111,3,0)|Out-Null
        [DbShellTest]::SendNotifyMessage($taskChoice,0x111,1,0)|Out-Null
        Start-Sleep -Milliseconds 150
        [DbShellTest]::SendNotifyMessage($taskWindow,0x111,9104,0)|Out-Null
        $taskChoice=[IntPtr]::Zero
        for($i=0;$i -lt 100;$i++){$taskChoice=[DbShellTest]::FindWindow('YYDbERChoice','論理名を編集する対象');if($taskChoice -ne [IntPtr]::Zero -and [DbShellTest]::GetDlgItem($taskChoice,2) -ne [IntPtr]::Zero){break};Start-Sleep -Milliseconds 30}
        if($taskChoice -eq [IntPtr]::Zero){throw 'ER logical name target did not open'}
        [DbShellTest]::SendNotifyMessage($taskChoice,0x111,1,0)|Out-Null
        $taskNameDialog=[IntPtr]::Zero
        for($i=0;$i -lt 100;$i++){$taskNameDialog=[DbShellTest]::FindWindow('YYDbName','論理名を編集');if($taskNameDialog -ne [IntPtr]::Zero -and [DbShellTest]::GetDlgItem($taskNameDialog,2) -ne [IntPtr]::Zero){break};Start-Sleep -Milliseconds 30}
        if($taskNameDialog -eq [IntPtr]::Zero){throw 'ER logical name editor did not open'}
        [DbShellTest]::TextMessage([DbShellTest]::FindChild($taskNameDialog,'Edit'),0xC,0,'顧客情報')|Out-Null
        [DbShellTest]::SendNotifyMessage($taskNameDialog,0x111,1,0)|Out-Null
        Start-Sleep -Milliseconds 150
        [DbShellTest]::ShowWindow($taskWindow,4)|Out-Null
        Snapshot $taskWindow 'er-logical-name'
        function Wait-ERDialog([string]$Class,[string]$Title) {
            for($i=0;$i -lt 100;$i++){$h=[DbShellTest]::FindWindow($Class,$Title);if($h -ne [IntPtr]::Zero -and [DbShellTest]::GetDlgItem($h,2) -ne [IntPtr]::Zero){return $h};Start-Sleep -Milliseconds 30}
            throw "ER dialog did not open: $Title"
        }
        [DbShellTest]::SendNotifyMessage($taskWindow,0x111,9117,0)|Out-Null
        $taskForm=Wait-ERDialog 'YYDbERForm' '列を編集'
        [DbShellTest]::TextMessage([DbShellTest]::GetDlgItem($taskForm,100),0xC,0,'note')|Out-Null
        [DbShellTest]::TextMessage([DbShellTest]::GetDlgItem($taskForm,101),0xC,0,'備考')|Out-Null
        [DbShellTest]::TextMessage([DbShellTest]::GetDlgItem($taskForm,102),0xC,0,'VARCHAR(40)')|Out-Null
        [DbShellTest]::TextMessage([DbShellTest]::GetDlgItem($taskForm,104),0xC,0,"'a;b'")|Out-Null
        [DbShellTest]::SendNotifyMessage($taskForm,0x111,1,0)|Out-Null
        Start-Sleep -Milliseconds 120
        [DbShellTest]::SendNotifyMessage($taskWindow,0x111,9123,0)|Out-Null
        $taskForm=Wait-ERDialog 'YYDbERForm' '索引を編集'
        [DbShellTest]::TextMessage([DbShellTest]::GetDlgItem($taskForm,100),0xC,0,'IX_note')|Out-Null
        [DbShellTest]::SendNotifyMessage($taskForm,0x111,1,0)|Out-Null
        $taskChoice=Wait-ERDialog 'YYDbERChoice' '索引を構成する列'
        $taskList=[DbShellTest]::GetDlgItem($taskChoice,10)
        $taskColumns=[DbShellTest]::SendMessage($taskList,0x18B,0,0).ToInt64()
        [DbShellTest]::SendMessage($taskList,0x185,1,($taskColumns-1))|Out-Null
        [DbShellTest]::SendNotifyMessage($taskChoice,0x111,1,0)|Out-Null
        Start-Sleep -Milliseconds 120
        [DbShellTest]::SendNotifyMessage($taskWindow,0x111,9114,0)|Out-Null
        $taskForm=Wait-ERDialog 'YYDbERForm' 'テーブルを追加'
        [DbShellTest]::TextMessage([DbShellTest]::GetDlgItem($taskForm,101),0xC,0,'CI_new_table')|Out-Null
        [DbShellTest]::TextMessage([DbShellTest]::GetDlgItem($taskForm,102),0xC,0,'新規テーブル')|Out-Null
        [DbShellTest]::SendNotifyMessage($taskForm,0x111,1,0)|Out-Null
        Start-Sleep -Milliseconds 120
        [DbShellTest]::SendNotifyMessage($taskWindow,0x111,9113,0)|Out-Null
        $taskTabs=[DbShellTest]::GetDlgItem($taskWindow,117)
        $taskTabCount=[DbShellTest]::SendMessage($taskTabs,0x1304,0,0).ToInt64()
        [DbShellTest]::SendNotifyMessage($taskWindow,0x111,9129,0)|Out-Null
        Start-Sleep -Milliseconds 180
        if([DbShellTest]::SendMessage($taskTabs,0x1304,0,0).ToInt64() -ne $taskTabCount+1){throw 'DDL did not open in a new SQL tab'}
        Snapshot $taskWindow 'er-ddl'
    }
    [DbShellTest]::SendNotifyMessage($taskWindow,0x111,9132,0)|Out-Null
    $taskNewER=Wait-VisibleDialog 'YYDbERChoice' '新しいERモデルの対象DB（接続不要）'
    $taskDbList=[DbShellTest]::GetDlgItem($taskNewER,10)
    if([DbShellTest]::SendMessage($taskDbList,0x18B,0,0).ToInt64() -ne 7){throw 'New ER model does not offer all database kinds'}
    [DbShellTest]::SendMessage($taskDbList,0x186,4,0)|Out-Null
    [DbShellTest]::SendNotifyMessage($taskNewER,0x111,1,0)|Out-Null
    # The modal dialog enables its owner before the command installs the model
    # and lays out the ER canvas. Wait for the resulting UI, not just EnableWindow.
    $taskNewReady=$false
    for($i=0;$i -lt 250;$i++) {
        $taskNewStatus=[Text.StringBuilder]::new(2048)
        [DbShellTest]::GetWindowText([DbShellTest]::GetDlgItem($taskWindow,205),$taskNewStatus,2048)|Out-Null
        if(![DbShellTest]::IsWindow($taskNewER) -and
           [DbShellTest]::IsWindowEnabled($taskWindow) -and
           $taskNewStatus.ToString().Contains('空のERモデル') -and
           ![DbShellTest]::IsWindowVisible([DbShellTest]::GetDlgItem($taskWindow,203)) -and
           ![DbShellTest]::IsWindowVisible([DbShellTest]::GetDlgItem($taskWindow,118))) {
            $taskNewReady=$true
            break
        }
        Start-Sleep -Milliseconds 20
    }
    if(!$taskNewReady){throw "Blank ER model was not created without a database connection. Status: $taskNewStatus`n$([DbShellTest]::Describe($taskProcess.Id))"}
    [DbShellTest]::ShowWindow($taskWindow,4)|Out-Null
    $taskDocumentTabs=[DbShellTest]::GetDlgItem($taskWindow,117)
    $taskDocumentCount=[DbShellTest]::SendMessage($taskDocumentTabs,0x1304,0,0).ToInt64()
    if([DbShellTest]::IsWindowVisible([DbShellTest]::GetDlgItem($taskWindow,203))){throw 'ER tab still shows SQL editor'}
    if([DbShellTest]::IsWindowVisible([DbShellTest]::GetDlgItem($taskWindow,118))){throw 'ER tab still shows SQL result tabs'}
    if([DbShellTest]::SendMessage($taskDocumentTabs,0x130B,0,0).ToInt64() -ne $taskDocumentCount-1){throw 'ER is not a document tab alongside SQL'}
    Snapshot $taskWindow 'er-new-empty'
    [DbShellTest]::SelectDocumentTab($taskWindow,$taskDocumentTabs,0)
    Start-Sleep -Milliseconds 60
    if(![DbShellTest]::IsWindowVisible([DbShellTest]::GetDlgItem($taskWindow,203))){throw 'Selecting SQL did not restore SQL editor'}
    Snapshot $taskWindow 'sql-document-tab'
    [DbShellTest]::SelectDocumentTab($taskWindow,$taskDocumentTabs,($taskDocumentCount-1))
    Start-Sleep -Milliseconds 60
    if([DbShellTest]::IsWindowVisible([DbShellTest]::GetDlgItem($taskWindow,203))){throw 'Selecting ER did not restore ER canvas'}
    [DbShellTest]::SendNotifyMessage($taskWindow,0x111,116,0)|Out-Null
    Start-Sleep -Milliseconds 80
    $taskNewCount=[DbShellTest]::SendMessage($taskDocumentTabs,0x1304,0,0).ToInt64()
    if($taskNewCount -ne $taskDocumentCount+1 -or [DbShellTest]::SendMessage($taskDocumentTabs,0x130B,0,0).ToInt64() -ne $taskNewCount-2){throw 'New SQL tab index interferes with ER tab'}
    if(![DbShellTest]::IsWindowVisible([DbShellTest]::GetDlgItem($taskWindow,203))){throw 'New SQL did not switch away from ER'}
    [DbShellTest]::SelectDocumentTab($taskWindow,$taskDocumentTabs,($taskNewCount-1))
    Start-Sleep -Milliseconds 60

    [DbShellTest]::SendNotifyMessage($taskWindow,0x111,9114,0)|Out-Null
    $taskNewTable=Wait-VisibleDialog 'YYDbERForm' 'テーブルを追加'
    foreach($taskField in @(@{Id=100;Text='public'},@{Id=101;Text='customer'},@{Id=102;Text='顧客'})) {
        [DbShellTest]::TextMessage([DbShellTest]::GetDlgItem($taskNewTable,$taskField.Id),0xC,0,$taskField.Text)|Out-Null
    }
    [DbShellTest]::SendNotifyMessage($taskNewTable,0x111,1,0)|Out-Null
    for($i=0;$i -lt 100 -and ![DbShellTest]::IsWindowEnabled($taskWindow);$i++){Start-Sleep -Milliseconds 20}
    $taskTabs=[DbShellTest]::GetDlgItem($taskWindow,117)
    $taskTabCount=[DbShellTest]::SendMessage($taskTabs,0x1304,0,0).ToInt64()
    [DbShellTest]::SendNotifyMessage($taskWindow,0x111,9129,0)|Out-Null
    for($i=0;$i -lt 100 -and [DbShellTest]::SendMessage($taskTabs,0x1304,0,0).ToInt64() -eq $taskTabCount;$i++){Start-Sleep -Milliseconds 20}
    if([DbShellTest]::SendMessage($taskTabs,0x1304,0,0).ToInt64() -ne $taskTabCount+1){throw 'A new ER model could not add a table and generate DDL offline'}
    Snapshot $taskWindow 'er-new-table-ddl'
    function Add-ERField([string]$Name,[string]$Type,[bool]$Important=$false) {
        [DbShellTest]::SendNotifyMessage($taskWindow,0x111,9117,0)|Out-Null
        $taskFieldForm=Wait-VisibleDialog 'YYDbERForm' '列を編集'
        if([DbShellTest]::SendMessage([DbShellTest]::GetDlgItem($taskFieldForm,102),0x146,0,0).ToInt64() -lt 5){throw 'Individual column type is not a populated dropdown'}
        [DbShellTest]::TextMessage([DbShellTest]::GetDlgItem($taskFieldForm,100),0xC,0,$Name)|Out-Null
        [DbShellTest]::TextMessage([DbShellTest]::GetDlgItem($taskFieldForm,102),0xC,0,$Type)|Out-Null
        [DbShellTest]::SendMessage([DbShellTest]::GetDlgItem($taskFieldForm,105),0xF1,[int]$Important,0)|Out-Null
        [DbShellTest]::SendNotifyMessage($taskFieldForm,0x111,1,0)|Out-Null
        for($i=0;$i -lt 100 -and ![DbShellTest]::IsWindowEnabled($taskWindow);$i++){Start-Sleep -Milliseconds 20}
        Start-Sleep -Milliseconds 40
    }
    Add-ERField 'customer_name' 'VARCHAR(100)' $true
    [DbShellTest]::SendNotifyMessage($taskWindow,0x111,9114,0)|Out-Null
    $taskOrderForm=Wait-VisibleDialog 'YYDbERForm' 'テーブルを追加'
    [DbShellTest]::TextMessage([DbShellTest]::GetDlgItem($taskOrderForm,101),0xC,0,'orders')|Out-Null
    [DbShellTest]::SendNotifyMessage($taskOrderForm,0x111,1,0)|Out-Null
    for($i=0;$i -lt 100 -and ![DbShellTest]::IsWindowEnabled($taskWindow);$i++){Start-Sleep -Milliseconds 20}
    Add-ERField 'customer_id' 'INTEGER'
    Add-ERField 'customer_name' 'VARCHAR(100)'
    Add-ERField 'detail' 'VARCHAR(100)'
    [DbShellTest]::SendNotifyMessage($taskWindow,0x111,9134,0)|Out-Null
    $taskMajor=Wait-VisibleDialog 'YYDbERChoice' '主要項目を選択（主キー・外部キーは常に表示）'
    [DbShellTest]::SendMessage([DbShellTest]::GetDlgItem($taskMajor,10),0x185,1,3)|Out-Null
    [DbShellTest]::SendNotifyMessage($taskMajor,0x111,1,0)|Out-Null
    Start-Sleep -Milliseconds 80
    [DbShellTest]::SendNotifyMessage($taskWindow,0x111,9135,0)|Out-Null
    $taskParent=Wait-VisibleDialog 'YYDbERChoice' '参照先テーブル'
    [DbShellTest]::SendMessage([DbShellTest]::GetDlgItem($taskParent,10),0x186,0,0)|Out-Null
    [DbShellTest]::SendNotifyMessage($taskParent,0x111,1,0)|Out-Null
    $taskColumns=Wait-VisibleDialog 'YYDbERChoice' '論理関係の参照先列（制約不要）'
    [DbShellTest]::SendMessage([DbShellTest]::GetDlgItem($taskColumns,10),0x185,1,1)|Out-Null
    [DbShellTest]::SendNotifyMessage($taskColumns,0x111,1,0)|Out-Null
    $taskSource=Wait-VisibleDialog 'YYDbERChoice' 'public.customer の customer_name に対応する参照元列'
    [DbShellTest]::SendMessage([DbShellTest]::GetDlgItem($taskSource,10),0x186,2,0)|Out-Null
    [DbShellTest]::SendNotifyMessage($taskSource,0x111,1,0)|Out-Null
    $taskRelationName=Wait-VisibleDialog 'YYDbName' '論理リレーション名（DB制約は作成しません）'
    [DbShellTest]::SendNotifyMessage($taskRelationName,0x111,1,0)|Out-Null
    Start-Sleep -Milliseconds 80
    [DbShellTest]::SendNotifyMessage($taskWindow,0x111,9136,0)|Out-Null
    $taskRecommend=Wait-VisibleDialog 'YYDbERChoice' '論理関係の推薦（意味を確認して選択）'
    $taskRecommendList=[DbShellTest]::GetDlgItem($taskRecommend,10)
    if([DbShellTest]::SendMessage($taskRecommendList,0x18B,0,0).ToInt64() -ne 1){throw 'Expected one unregistered logical relation recommendation'}
    if([DbShellTest]::SendMessage($taskRecommendList,0x190,0,0).ToInt64() -ne 0){throw 'Recommendations must require explicit selection'}
    Snapshot $taskRecommend 'er-recommendations'
    [DbShellTest]::SendMessage($taskRecommendList,0x185,1,0)|Out-Null
    [DbShellTest]::SendNotifyMessage($taskRecommend,0x111,1,0)|Out-Null
    Start-Sleep -Milliseconds 80
    $taskRelationStatus=[Text.StringBuilder]::new(2048)
    [DbShellTest]::GetWindowText([DbShellTest]::GetDlgItem($taskWindow,205),$taskRelationStatus,2048)|Out-Null
    if(!$taskRelationStatus.ToString().Contains('論理リレーションを1件登録')){throw 'Recommended logical relation was not registered'}
    [DbShellTest]::SendNotifyMessage($taskWindow,0x111,9133,0)|Out-Null
    [DbShellTest]::SendNotifyMessage($taskWindow,0x111,9113,0)|Out-Null
    Snapshot $taskWindow 'er-logical-important'
    [DbShellTest]::SendNotifyMessage($taskWindow,0x111,9138,0)|Out-Null
    $taskSizeForm=Wait-VisibleDialog 'YYDbERForm' 'テーブルのサイズを指定'
    [DbShellTest]::TextMessage([DbShellTest]::GetDlgItem($taskSizeForm,100),0xC,0,'660')|Out-Null
    [DbShellTest]::TextMessage([DbShellTest]::GetDlgItem($taskSizeForm,101),0xC,0,'220')|Out-Null
    [DbShellTest]::SendNotifyMessage($taskSizeForm,0x111,1,0)|Out-Null
    for($i=0;$i -lt 100 -and ![DbShellTest]::IsWindowEnabled($taskWindow);$i++){Start-Sleep -Milliseconds 20}
    Snapshot $taskWindow 'er-manual-size'
    [DbShellTest]::SendNotifyMessage($taskWindow,0x111,9139,0)|Out-Null
    Snapshot $taskWindow 'er-auto-size'
    # Spreadsheet column editor: real Win32 controls, Excel TSV clipboard, batch apply and cancellation.
    function Open-ERColumns {
        [DbShellTest]::SendNotifyMessage($taskWindow,0x111,9141,0)|Out-Null
        Wait-VisibleDialog 'YYDbERColumns' '列定義をまとめて編集 — orders'
    }
    function Read-ColumnClipboard([IntPtr]$Grid) {
        [DbShellTest]::SendMessage($Grid,0x301,0,0)|Out-Null
        (Get-Clipboard -Raw).TrimEnd("`r","`n")
    }
    $taskColumnDialog=Open-ERColumns
    $taskColumnGrid=[DbShellTest]::GetDlgItem($taskColumnDialog,100)
    $taskColumnInput=[DbShellTest]::GetDlgItem($taskColumnGrid,101)
    if((Read-ColumnClipboard $taskColumnGrid) -ne 'id'){throw 'Column grid did not load the selected table'}
    [DbShellTest]::SendMessage($taskColumnGrid,0x100,113,0)|Out-Null
    if(![DbShellTest]::IsWindowVisible($taskColumnInput)){throw 'F2 did not open the cell editor'}
    [DbShellTest]::TextMessage($taskColumnInput,0xC,0,'order_key')|Out-Null
    [DbShellTest]::SendMessage($taskColumnGrid,0x100,9,0)|Out-Null
    # Editable native type dropdown for PostgreSQL; candidate selection and typed parameters survive.
    [DbShellTest]::SendMessage($taskColumnGrid,0x100,39,0)|Out-Null
    [DbShellTest]::SendMessage($taskColumnGrid,0x100,115,0)|Out-Null
    $taskTypeCombo=[DbShellTest]::GetDlgItem($taskColumnGrid,102)
    if(![DbShellTest]::IsWindowVisible($taskTypeCombo)){throw 'Type editor is not an editable dropdown'}
    if([DbShellTest]::SendMessage($taskTypeCombo,0x157,0,0).ToInt64() -ne 1){throw 'F4 did not open type candidates'}
    Snapshot $taskColumnDialog 'er-column-type-dropdown'
    $taskTypeIndex=[DbShellTest]::TextMessage($taskTypeCombo,0x158,-1,'JSONB').ToInt64()
    if($taskTypeIndex -lt 0){throw 'PostgreSQL JSONB type candidate is missing'}
    if([DbShellTest]::TextMessage($taskTypeCombo,0x158,-1,'VARCHAR2(255 CHAR)').ToInt64() -ge 0){throw 'Oracle type leaked into PostgreSQL suggestions'}
    [DbShellTest]::SendMessage($taskTypeCombo,0x14E,$taskTypeIndex,0)|Out-Null
    [DbShellTest]::SendMessage($taskColumnGrid,0x100,9,0)|Out-Null
    [DbShellTest]::SendMessage($taskColumnGrid,0x100,37,0)|Out-Null
    if((Read-ColumnClipboard $taskColumnGrid) -ne 'JSONB'){throw 'Selected type was not committed'}
    # Clipboard into a type cell remains unrestricted, and F2 preserves the precise pasted value.
    Set-Clipboard -Value 'NUMERIC(23,7)'
    [DbShellTest]::SendMessage($taskColumnGrid,0x302,0,0)|Out-Null
    [DbShellTest]::SendMessage($taskColumnGrid,0x100,113,0)|Out-Null
    $taskTypeText=[Text.StringBuilder]::new(256)
    [DbShellTest]::ReadMessage($taskTypeCombo,0xD,256,$taskTypeText)|Out-Null
    if($taskTypeText.ToString() -ne 'NUMERIC(23,7)'){throw 'Type dropdown overwrote pasted parameters'}
    [DbShellTest]::TextMessage($taskTypeCombo,0xC,0,'INTEGER')|Out-Null
    Snapshot $taskColumnDialog 'er-column-type-editor'
    [DbShellTest]::SendMessage($taskColumnGrid,0x100,9,0)|Out-Null
    [DbShellTest]::SendMessage($taskColumnGrid,0x100,36,0)|Out-Null
    for($taskRow=0;$taskRow -lt 4;$taskRow++){[DbShellTest]::SendMessage($taskColumnGrid,0x100,40,0)|Out-Null}
    $taskColumnTsv="extra_code`t追加コード`tVARCHAR(40)`ttrue`t'ABC'`ttrue`r`nextra_count`t追加件数`tINTEGER`tfalse`t0`tfalse"
    Set-Clipboard -Value $taskColumnTsv
    [DbShellTest]::SendMessage($taskColumnGrid,0x302,0,0)|Out-Null
    if((Read-ColumnClipboard $taskColumnGrid) -ne $taskColumnTsv){throw 'Excel rectangle paste/copy did not preserve all cells'}
    [DbShellTest]::SendMessage($taskColumnDialog,0x111,16,0)|Out-Null
    [DbShellTest]::SendMessage($taskColumnDialog,0x111,17,0)|Out-Null
    if((Read-ColumnClipboard $taskColumnGrid) -ne $taskColumnTsv){throw 'Column grid undo/redo lost pasted values'}
    [DbShellTest]::SendMessage($taskColumnDialog,0x111,12,0)|Out-Null
    if((Read-ColumnClipboard $taskColumnGrid) -ne $taskColumnTsv){throw 'Moving the selected rows lost values'}
    Snapshot $taskColumnDialog 'er-column-grid'
    [DbShellTest]::ShowWindow($taskColumnDialog,3)|Out-Null
    Snapshot $taskColumnDialog 'er-column-grid-maximized'
    [DbShellTest]::ShowWindow($taskColumnDialog,9)|Out-Null
    [DbShellTest]::SendNotifyMessage($taskColumnDialog,0x111,1,0)|Out-Null
    for($i=0;$i -lt 100 -and ![DbShellTest]::IsWindowEnabled($taskWindow);$i++){Start-Sleep -Milliseconds 20}
    if(![DbShellTest]::IsWindowEnabled($taskWindow)){throw 'Valid batch column definitions did not apply'}
    $taskColumnDialog=Open-ERColumns
    $taskColumnGrid=[DbShellTest]::GetDlgItem($taskColumnDialog,100)
    if((Read-ColumnClipboard $taskColumnGrid) -ne 'order_key'){throw 'Renamed primary key was not applied'}
    for($taskRow=0;$taskRow -lt 3;$taskRow++){[DbShellTest]::SendMessage($taskColumnGrid,0x100,40,0)|Out-Null}
    if((Read-ColumnClipboard $taskColumnGrid) -ne 'extra_code'){throw 'Pasted rows were not reordered and persisted'}
    [DbShellTest]::SendMessage($taskColumnDialog,0x111,11,0)|Out-Null
    [DbShellTest]::SendNotifyMessage($taskColumnDialog,0x111,2,0)|Out-Null
    Start-Sleep -Milliseconds 60
    $taskColumnDialog=Open-ERColumns
    $taskColumnGrid=[DbShellTest]::GetDlgItem($taskColumnDialog,100)
    for($taskRow=0;$taskRow -lt 3;$taskRow++){[DbShellTest]::SendMessage($taskColumnGrid,0x100,40,0)|Out-Null}
    if((Read-ColumnClipboard $taskColumnGrid) -ne 'extra_code'){throw 'Cancel did not discard column deletion'}
    [DbShellTest]::SendNotifyMessage($taskColumnDialog,0x111,2,0)|Out-Null
    Start-Sleep -Milliseconds 60
    [DbShellTest]::SendNotifyMessage($taskWindow,0x111,9130,0)|Out-Null
    Start-Sleep -Milliseconds 60
    $taskColumnDialog=Open-ERColumns
    $taskColumnGrid=[DbShellTest]::GetDlgItem($taskColumnDialog,100)
    if((Read-ColumnClipboard $taskColumnGrid) -ne 'id'){throw 'ER undo did not restore the entire batch edit'}
    [DbShellTest]::SendNotifyMessage($taskColumnDialog,0x111,2,0)|Out-Null
    Start-Sleep -Milliseconds 60
    [DbShellTest]::SendNotifyMessage($taskWindow,0x111,9131,0)|Out-Null
    Start-Sleep -Milliseconds 60
    $taskColumnDialog=Open-ERColumns
    $taskColumnGrid=[DbShellTest]::GetDlgItem($taskColumnDialog,100)
    if((Read-ColumnClipboard $taskColumnGrid) -ne 'order_key'){throw 'ER redo did not restore the batch edit'}
    # In-use key deletion is rejected without discarding the edit buffer.
    [DbShellTest]::SendMessage($taskColumnDialog,0x111,11,0)|Out-Null
    [DbShellTest]::SendNotifyMessage($taskColumnDialog,0x111,1,0)|Out-Null
    $taskColumnError=Wait-VisibleDialog '#32770' '列定義を確認してください'
    Snapshot $taskColumnError 'er-column-validation'
    [DbShellTest]::PostMessage($taskColumnError,0x10,0,0)|Out-Null
    for($i=0;$i -lt 100 -and [DbShellTest]::IsWindow($taskColumnError);$i++){Start-Sleep -Milliseconds 20}
    if([DbShellTest]::IsWindow($taskColumnError)){throw "Column validation error did not close`n$([DbShellTest]::Describe($taskProcess.Id))"}
    Start-Sleep -Milliseconds 60
    [DbShellTest]::SendMessage($taskColumnDialog,0x111,16,0)|Out-Null
    [DbShellTest]::SendMessage($taskColumnGrid,0x201,1,(5 -bor (5 -shl 16)))|Out-Null
    [DbShellTest]::SendMessage($taskColumnGrid,0x202,0,(5 -bor (5 -shl 16)))|Out-Null
    $taskAllColumns=Read-ColumnClipboard $taskColumnGrid
    if(!$taskAllColumns.Contains('order_key') -or !$taskAllColumns.Contains('extra_count')){throw 'Undo after validation error or rectangular select-all failed'}
    $taskAllColumns | Set-Content (Join-Path $taskOutput 'er-column-grid.tsv') -Encoding utf8
    [DbShellTest]::SendNotifyMessage($taskColumnDialog,0x111,2,0)|Out-Null
    Start-Sleep -Milliseconds 60
    [DbShellTest]::SendNotifyMessage($taskWindow,0x111,9137,0)|Out-Null
    $taskMarkdownSave=Wait-VisibleDialog '#32770' 'Markdown設計書の出力先'
    [DbShellTest]::PostMessage($taskMarkdownSave,0x10,0,0)|Out-Null
    Start-Sleep -Milliseconds 100
    # Each offline ER database kind supplies native type choices in both editors.
    foreach($taskTypeCase in @(
        @{Index=0;Name='sqlite';Type='TEXT'},@{Index=1;Name='duckdb';Type='HUGEINT'},
        @{Index=2;Name='oracle';Type='VARCHAR2(255 CHAR)'},@{Index=3;Name='sqlserver';Type='NVARCHAR(MAX)'},
        @{Index=4;Name='postgresql';Type='JSONB'},@{Index=5;Name='mysql';Type='INT UNSIGNED'},
        @{Index=6;Name='db2';Type='DECFLOAT(34)'})) {
        [DbShellTest]::SendNotifyMessage($taskWindow,0x111,9132,0)|Out-Null
        $taskKindForm=Wait-VisibleDialog 'YYDbERChoice' '新しいERモデルの対象DB（接続不要）'
        [DbShellTest]::SendMessage([DbShellTest]::GetDlgItem($taskKindForm,10),0x186,$taskTypeCase.Index,0)|Out-Null
        [DbShellTest]::SendNotifyMessage($taskKindForm,0x111,1,0)|Out-Null
        Wait-DialogClosed $taskKindForm
        Start-Sleep -Milliseconds 60
        [DbShellTest]::SendNotifyMessage($taskWindow,0x111,9114,0)|Out-Null
        $taskNewTable=Wait-VisibleDialog 'YYDbERForm' 'テーブルを追加'
        [DbShellTest]::SendNotifyMessage($taskNewTable,0x111,1,0)|Out-Null
        Wait-DialogClosed $taskNewTable
        Start-Sleep -Milliseconds 60
        [DbShellTest]::SendNotifyMessage($taskWindow,0x111,9141,0)|Out-Null
        $taskTypesDialog=Wait-VisibleDialog 'YYDbERColumns' '列定義をまとめて編集 — table_1'
        $taskTypesGrid=[DbShellTest]::GetDlgItem($taskTypesDialog,100)
        [DbShellTest]::SendMessage($taskTypesGrid,0x100,39,0)|Out-Null
        [DbShellTest]::SendMessage($taskTypesGrid,0x100,39,0)|Out-Null
        [DbShellTest]::SendMessage($taskTypesGrid,0x100,115,0)|Out-Null
        $taskTypesCombo=[DbShellTest]::GetDlgItem($taskTypesGrid,102)
        if([DbShellTest]::TextMessage($taskTypesCombo,0x158,-1,$taskTypeCase.Type).ToInt64() -lt 0){throw "Missing $($taskTypeCase.Name) native candidate"}
        Snapshot $taskTypesDialog "er-types-$($taskTypeCase.Name)"
        [DbShellTest]::SendNotifyMessage($taskTypesDialog,0x111,2,0)|Out-Null
        Wait-DialogClosed $taskTypesDialog
        Start-Sleep -Milliseconds 60
        [DbShellTest]::SendNotifyMessage($taskWindow,0x111,9117,0)|Out-Null
        $taskSingleType=Wait-VisibleDialog 'YYDbERForm' '列を編集'
        if([DbShellTest]::TextMessage([DbShellTest]::GetDlgItem($taskSingleType,102),0x158,-1,$taskTypeCase.Type).ToInt64() -lt 0){throw "Missing $($taskTypeCase.Name) individual column candidate"}
        [DbShellTest]::SendNotifyMessage($taskSingleType,0x111,2,0)|Out-Null
        Wait-DialogClosed $taskSingleType
        Start-Sleep -Milliseconds 60
    }
    [DbShellTest]::TextMessage([DbShellTest]::GetDlgItem($taskWindow,202),0xC,0,$taskTarget)|Out-Null
    [DbShellTest]::SendNotifyMessage($taskWindow,0x111,122,0)|Out-Null
    $taskDialog=[IntPtr]::Zero
    for($i=0;$i -lt 100;$i++){$taskDialog=[DbShellTest]::FindWindow('YYDbConnection','接続先の設定');if($taskDialog -ne [IntPtr]::Zero -and [DbShellTest]::IsWindowVisible($taskDialog)){break};Start-Sleep -Milliseconds 50}
    if($taskDialog -eq [IntPtr]::Zero){throw "Save connection dialog did not appear: $([DbShellTest]::Describe($taskProcess.Id))"}
    $taskEdit=[IntPtr]::Zero
    for($i=0;$i -lt 100;$i++){$taskEdit=[DbShellTest]::GetDlgItem($taskDialog,100);if($taskEdit -ne [IntPtr]::Zero){break};Start-Sleep -Milliseconds 20}
    if($taskEdit -eq [IntPtr]::Zero){throw 'Name edit did not appear'}
    for($i=0;$i -lt 100 -and [DbShellTest]::GetDlgItem($taskDialog,2) -eq [IntPtr]::Zero;$i++){Start-Sleep -Milliseconds 20}
    Start-Sleep -Milliseconds 100
    [DbShellTest]::TextMessage($taskEdit,0xC,0,'画面検証用 SQLite')|Out-Null
    $taskName=[Text.StringBuilder]::new(256);[DbShellTest]::ReadMessage($taskEdit,0xD,256,$taskName)|Out-Null
    if($taskName.ToString() -ne '画面検証用 SQLite'){throw "Name edit was not updated: $taskName"}
    [DbShellTest]::TextMessage([DbShellTest]::GetDlgItem($taskDialog,102),0xC,0,$taskTarget)|Out-Null
    Snapshot $taskDialog 'connection-form'
    [DbShellTest]::SendNotifyMessage($taskDialog,0x111,1,0)|Out-Null
    for($i=0;$i -lt 100 -and !(Test-Path $env:YYDBMANAGER_CONNECTIONS_FILE);$i++){Start-Sleep -Milliseconds 50}
    if(!(Test-Path $env:YYDBMANAGER_CONNECTIONS_FILE)){throw "Connection profile was not saved: $([DbShellTest]::Describe($taskProcess.Id))"}
    foreach($taskHidden in @(201,202,124)){if([DbShellTest]::IsWindowVisible([DbShellTest]::GetDlgItem($taskWindow,$taskHidden))){throw 'Connection details are still visible in the main window'}}
    if([DbShellTest]::GetDlgItem($taskWindow,125) -ne [IntPtr]::Zero){throw 'Global ambiguous refresh button still exists'}
    [DbShellTest]::SendNotifyMessage($taskWindow,0x111,101,0)|Out-Null
    Start-Sleep -Milliseconds 600
    $taskTree=[DbShellTest]::GetDlgItem($taskWindow,210)
    [DbShellTest]::SendNotifyMessage($taskWindow,0x7B,$taskTree,-1)|Out-Null
    $taskPopup=[IntPtr]::Zero
    for($i=0;$i -lt 100;$i++){$taskPopup=[DbShellTest]::FindWindow('#32768',$null);if($taskPopup -ne [IntPtr]::Zero){break};Start-Sleep -Milliseconds 20}
    if($taskPopup -eq [IntPtr]::Zero){throw 'Explorer context menu did not open'}
    $taskPopupMenu=[DbShellTest]::SendMessage($taskPopup,0x1E1,0,0)
    $taskMenuLabels=''
    for($i=0;$i -lt [DbShellTest]::GetMenuItemCount($taskPopupMenu);$i++){$taskLabel=[Text.StringBuilder]::new(256);[DbShellTest]::GetMenuString($taskPopupMenu,$i,$taskLabel,256,0x400)|Out-Null;$taskMenuLabels+=$taskLabel.ToString()}
    if(!$taskMenuLabels.Contains('オブジェクト一覧を更新')){throw "Explorer refresh entry missing: $taskMenuLabels"}
    $taskPerformanceMenu=[IntPtr]::Zero
    for($i=0;$i -lt [DbShellTest]::GetMenuItemCount($taskPopupMenu);$i++) {
        $taskLabel=[Text.StringBuilder]::new(256)
        [DbShellTest]::GetMenuString($taskPopupMenu,$i,$taskLabel,256,0x400)|Out-Null
        if($taskLabel.ToString().Contains('性能分析SQL（SQLite）')){$taskPerformanceMenu=[DbShellTest]::GetSubMenu($taskPopupMenu,$i)}
    }
    if($taskPerformanceMenu -eq [IntPtr]::Zero -or [DbShellTest]::GetMenuItemCount($taskPerformanceMenu) -ne 7){throw 'SQLite diagnostic submenu missing'}
    $taskPerformanceCommand=[DbShellTest]::GetMenuItemID($taskPerformanceMenu,0)
    Snapshot $taskPopup 'explorer-context'
    [DbShellTest]::PostMessage($taskWindow,0x1F,0,0)|Out-Null
    Start-Sleep -Milliseconds 80
    $taskDiagnosticTabs=[DbShellTest]::GetDlgItem($taskWindow,117)
    $taskDiagnosticBefore=[DbShellTest]::SendMessage($taskDiagnosticTabs,0x1304,0,0).ToInt64()
    [DbShellTest]::SendNotifyMessage($taskWindow,0x111,$taskPerformanceCommand,0)|Out-Null
    for($i=0;$i -lt 100 -and [DbShellTest]::SendMessage($taskDiagnosticTabs,0x1304,0,0).ToInt64() -eq $taskDiagnosticBefore;$i++){Start-Sleep -Milliseconds 20}
    if([DbShellTest]::SendMessage($taskDiagnosticTabs,0x1304,0,0).ToInt64() -ne $taskDiagnosticBefore+1){throw 'Diagnostic SQL did not create a tab'}
    $taskDiagnosticStatus=[Text.StringBuilder]::new(2048)
    [DbShellTest]::ReadMessage([DbShellTest]::GetDlgItem($taskWindow,205),0xD,2048,$taskDiagnosticStatus)|Out-Null
    if(!$taskDiagnosticStatus.ToString().Contains('未実行')){throw 'Diagnostic template selection executed SQL'}
    Snapshot $taskWindow 'diagnostic-template'
    [DbShellTest]::SendNotifyMessage($taskWindow,0x111,102,0)|Out-Null
    for($i=0;$i -lt 150 -and [DbShellTest]::SendMessage([DbShellTest]::GetDlgItem($taskWindow,118),0x1304,0,0).ToInt64() -eq 0;$i++){Start-Sleep -Milliseconds 20}
    if([DbShellTest]::SendMessage([DbShellTest]::GetDlgItem($taskWindow,118),0x1304,0,0).ToInt64() -ne 1){throw 'Diagnostic SQL could not be executed explicitly on its connection'}
    [DbShellTest]::ShowWindow($taskWindow,4)|Out-Null
    Snapshot $taskWindow 'normal'
    [DbShellTest]::ShowWindow($taskWindow,3)|Out-Null
    Snapshot $taskWindow 'maximized'
    [DbShellTest]::ShowWindow($taskWindow,9)|Out-Null
    Snapshot $taskWindow 'restored'
    [DbShellTest]::PostMessage($taskWindow,0x10,0,0)|Out-Null
    if(!$taskProcess.WaitForExit(5000)){throw 'App did not close'}
    $taskProcess=Start-Process @taskStart
    $taskWindow=Wait-Frame
    $taskCombo=[DbShellTest]::GetDlgItem($taskWindow,124)
    if([DbShellTest]::SendMessage($taskCombo,0x146,0,0).ToInt64() -ne 1){throw 'Saved connection did not reload'}
    [DbShellTest]::SendMessage($taskCombo,0x14E,0,0)|Out-Null
    [DbShellTest]::SendMessage($taskWindow,0x111,(124 -bor (1 -shl 16)),$taskCombo)|Out-Null
    [DbShellTest]::SendNotifyMessage($taskWindow,0x111,122,0)|Out-Null
    $taskDialog=[IntPtr]::Zero
    for($i=0;$i -lt 100;$i++){$taskDialog=[DbShellTest]::FindWindow('YYDbConnection','接続先の設定');if($taskDialog -ne [IntPtr]::Zero -and [DbShellTest]::IsWindowVisible($taskDialog)){break};Start-Sleep -Milliseconds 20}
    if($taskDialog -eq [IntPtr]::Zero){throw 'Edit connection dialog did not reopen'}
    $taskText=[Text.StringBuilder]::new(2048)
    [DbShellTest]::ReadMessage([DbShellTest]::GetDlgItem($taskDialog,102),0xD,2048,$taskText)|Out-Null
    if($taskText.ToString() -ne $taskTarget){throw 'Reloaded connection target differs'}
    [DbShellTest]::SendNotifyMessage($taskDialog,0x111,2,0)|Out-Null
    Start-Sleep -Milliseconds 60
    [DbShellTest]::SendNotifyMessage($taskWindow,0x111,123,0)|Out-Null
    $taskConfirm=[IntPtr]::Zero
    for($i=0;$i -lt 100;$i++){$taskConfirm=[DbShellTest]::FindWindow('#32770','接続設定');if($taskConfirm -ne [IntPtr]::Zero -and [DbShellTest]::GetDlgItem($taskConfirm,6) -ne [IntPtr]::Zero){break};Start-Sleep -Milliseconds 20}
    if($taskConfirm -eq [IntPtr]::Zero){throw 'Delete confirmation did not appear'}
    [DbShellTest]::SendNotifyMessage([DbShellTest]::GetDlgItem($taskConfirm,6),0xF5,0,0)|Out-Null
    for($i=0;$i -lt 100 -and [DbShellTest]::SendMessage($taskCombo,0x146,0,0).ToInt64() -ne 0;$i++){Start-Sleep -Milliseconds 20}
    if([DbShellTest]::SendMessage($taskCombo,0x146,0,0).ToInt64() -ne 0){throw 'Saved connection was not deleted'}
    [DbShellTest]::SendNotifyMessage($taskWindow,0x111,121,0)|Out-Null
    $taskDialog=[IntPtr]::Zero
    for($i=0;$i -lt 100;$i++){$taskDialog=[DbShellTest]::FindWindow('YYDbConnection','接続先の設定');if($taskDialog -ne [IntPtr]::Zero -and [DbShellTest]::IsWindowVisible($taskDialog)){break};Start-Sleep -Milliseconds 20}
    $taskKind=[DbShellTest]::GetDlgItem($taskDialog,101)
    [DbShellTest]::SendMessage($taskKind,0x14E,4,0)|Out-Null
    [DbShellTest]::SendMessage($taskDialog,0x111,(101 -bor (1 -shl 16)),$taskKind)|Out-Null
    foreach($taskField in @(@{Id=100;Text='検証 PostgreSQL'},@{Id=103;Text='PostgreSQL Unicode'},@{Id=104;Text='db.example'},@{Id=106;Text='sales'},@{Id=107;Text='developer'},@{Id=108;Text='test-secret;}'})){
        [DbShellTest]::TextMessage([DbShellTest]::GetDlgItem($taskDialog,$taskField.Id),0xC,0,$taskField.Text)|Out-Null
    }
    $taskPort=[Text.StringBuilder]::new(64);[DbShellTest]::ReadMessage([DbShellTest]::GetDlgItem($taskDialog,105),0xD,64,$taskPort)|Out-Null
    if($taskPort.ToString() -ne '5432'){throw 'Default PostgreSQL port is missing'}
    if([DbShellTest]::SendMessage([DbShellTest]::GetDlgItem($taskDialog,108),0xD2,0,0).ToInt64() -eq 0){throw 'Password is not masked'}
    Snapshot $taskDialog 'external-connection-form'
    [DbShellTest]::SendNotifyMessage($taskDialog,0x111,1,0)|Out-Null
    Start-Sleep -Milliseconds 80
    [DbShellTest]::SendNotifyMessage($taskWindow,0x111,122,0)|Out-Null
    $taskDialog=[IntPtr]::Zero
    for($i=0;$i -lt 100;$i++){$taskDialog=[DbShellTest]::FindWindow('YYDbConnection','接続先の設定');if($taskDialog -ne [IntPtr]::Zero -and [DbShellTest]::IsWindowVisible($taskDialog)){break};Start-Sleep -Milliseconds 20}
    if(![DbShellTest]::IsWindowVisible([DbShellTest]::GetDlgItem($taskDialog,104))){throw 'Structured connection fields were not retained'}
    $taskHost=[Text.StringBuilder]::new(256);[DbShellTest]::ReadMessage([DbShellTest]::GetDlgItem($taskDialog,104),0xD,256,$taskHost)|Out-Null
    if($taskHost.ToString() -ne 'db.example'){throw 'Server name was not retained'}
    [DbShellTest]::SendNotifyMessage($taskDialog,0x111,2,0)|Out-Null
    Start-Sleep -Milliseconds 60

    [DbShellTest]::PostMessage($taskWindow,0x10,0,0)|Out-Null
    if(!$taskProcess.WaitForExit(5000)){throw 'App did not close'}
    'Menus, offline help/F1, Oracle/Db2 instructions, SQL/ER document tabs/ER spreadsheet/7 DB type dropdowns/Excel clipboard/batch validation/undo/resize, connection dialogs, encrypted persistence/reload OK' | Set-Content (Join-Path $taskOutput 'shell-test.txt')
    Get-Content (Join-Path $taskOutput 'shell-test.txt')
} finally {
    if($taskProcess -and !$taskProcess.HasExited){Stop-Process -Id $taskProcess.Id}
    Set-Clipboard -Value $taskOldClipboard
    $env:YYDBMANAGER_CONNECTIONS_FILE=$taskOldConfig
    [DbShellTest]::SetThreadDpiAwarenessContext($taskOldDpi)|Out-Null
}
