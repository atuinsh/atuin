extern crate termion;

use termion::{clear, color, cursor};

use std::{time, thread};

const COMMUNISM: &'static str = r#"
              !#########       #                 
            !########!          ##!              
         !########!               ###            
      !##########                  ####          
    ######### #####                ######        
     !###!      !####!              ######       
       !           #####            ######!      
                     !####!         #######      
                        #####       #######      
                          !####!   #######!      
                             ####!########       
          ##                   ##########        
        ,######!          !#############         
      ,#### ########################!####!       
    ,####'     ##################!'    #####     
  ,####'            #######              !####!  
 ####'                                      #####
 ~##                                          ##~
"#;

fn main() {
    let mut state = 0;

    println!("\n{}{}{}{}{}{}",
             cursor::Hide,
             clear::All,
             cursor::Goto(1, 1),
             color::Fg(color::Black),
             color::Bg(color::Red),
             COMMUNISM);
    loop {
        println!("{}{}           ☭ GAY ☭ SPACE ☭ COMMUNISM ☭           ",
                 cursor::Goto(1, 1),
                 color::Bg(color::AnsiValue(state)));
        println!("{}{}             WILL PREVAIL, COMRADES!             ",
                 cursor::Goto(1, 20),
                 color::Bg(color::AnsiValue(state)));

        state += 1;
        state %= 8;

        thread::sleep(time::Duration::from_millis(90));
    }
}
