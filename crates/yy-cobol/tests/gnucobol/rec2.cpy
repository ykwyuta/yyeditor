      * yy-cobol と GnuCOBOL の比べ合わせ（COMP-5・COMP-1・COMP-2 は
      * 機械の並び = x86 ではリトルエンディアン）
       01  NAT-REC.
           05  N-ID              PIC 9(4).
           05  N-C5A             PIC S9(4) COMP-5.
           05  N-C5B             PIC S9(9) COMP-5.
           05  N-C5C             PIC 9(18) COMP-5.
           05  N-F1              COMP-1.
           05  N-F2              USAGE COMP-2.
