      * yy-cobol と GnuCOBOL の比べ合わせ（DISPLAY の項目だけ。
      * -fsign=EBCDIC で書いたものを iconv で EBCDIC にして比べる）
       01  EBC-REC.
           05  E-ID              PIC 9(4).
           05  E-NAME            PIC X(10).
           05  E-Z1              PIC S9(5)V99.
           05  E-Z2              PIC S9(3) SIGN LEADING.
           05  E-Z3              PIC S9(3) SIGN LEADING SEPARATE.
           05  E-Z4              PIC S9(3) SIGN TRAILING SEPARATE.
           05  E-Z5              PIC 9(5).
           05  E-E1              PIC ZZ,ZZ9.99-.
           05  E-E2              PIC $$$9.99CR.
           05  E-E3              PIC +ZZ9.
