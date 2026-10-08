      * yy-cobol と GnuCOBOL の比べ合わせ（2 進数はビッグエンディアン）
       01  TEST-REC.
           05  T-ID              PIC 9(4).
           05  T-NAME            PIC X(12).
           05  T-ALPHA           PIC A(5).
           05  T-JUST            PIC X(6) JUSTIFIED RIGHT.
           05  T-STAT            PIC X VALUE 'A'.
               88  T-ACTIVE      VALUE 'A'.
           05  T-Z1              PIC S9(5)V99.
           05  T-Z2              PIC 9(3) VALUE ZERO.
           05  T-Z3              PIC S9(3) SIGN IS LEADING.
           05  T-Z4              PIC S9(3) SIGN LEADING SEPARATE.
           05  T-Z5              PIC S9(3)
                                 SIGN TRAILING SEPARATE CHARACTER.
           05  T-Z6              PIC S9(15)V9(3).
           05  T-P1              PIC S9(7)V99 COMP-3.
           05  T-P2              PIC 9(4) PACKED-DECIMAL.
           05  T-P3              PIC S9(18) COMP-3.
           05  T-P4              PIC S9(3)V9(3) USAGE COMPUTATIONAL-3.
           05  T-B1              PIC S9(4) COMP.
           05  T-B2              PIC 9(8) BINARY.
           05  T-B3              PIC S9(18) COMP-4.
           05  T-B4              PIC S9(5)V99 COMP.
           05  T-E1              PIC ZZ,ZZ9.99-.
           05  T-E2              PIC $$$,$$9.99CR.
           05  T-E3              PIC ***9.99.
           05  T-E4              PIC +++9.
           05  T-E5              PIC 9(4)/99/99.
           05  T-E6              PIC ZZZ9 BLANK WHEN ZERO.
           05  T-E7              PIC -ZZZ9.
           05  T-E8              PIC 99B99B99.
           05  T-E9              PIC ZZ9.99DB.
           05  T-PS              PIC 99PPP.
           05  T-VP              PIC SVPP99.
           05  T-GRP.
               10  T-OCC         OCCURS 3 TIMES.
                   15  T-OA      PIC X(2).
                   15  T-OB      PIC S9(3) COMP-3.
           05  T-RED-SRC         PIC X(8).
           05  T-RED REDEFINES T-RED-SRC
                                 PIC 9(8).
           05  FILLER            PIC X(2).
           05  T-PAD             PIC X.
           05  T-SYNC            PIC S9(9) COMP SYNC.
           05  T-USG             USAGE COMP-3.
               10  T-U1          PIC S9(3).
               10  T-U2          PIC 9(2).
           05  T-SGN             SIGN IS LEADING SEPARATE.
               10  T-S1          PIC S9(2).
